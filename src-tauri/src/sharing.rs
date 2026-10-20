use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::State;
use unsub_library::{AlbumSummary, Library, MediaKind, PlaylistKind, SortKey, Track, TrackQuery};
use unsub_playback::{Target, TranscodeOptions};
use unsub_subsonic::{
    Album, AlbumListKind, Artist, Capabilities, Genre, MusicSource, Playlist, PlaylistEdit, SearchResult, Server, ServerConfig, Song,
    Starred, Stream, StreamFormat, Targets,
};

use crate::error::{CommandError, CommandResult};

type SourceResult<T> = unsub_subsonic::Result<T>;

/// A star on a phone is a five-star rating here: the library's own favourites, both ways.
const STARRED_RATING: u8 = 100;

/// What a phone gets when it asks for a format but no rate: good on cellular, fine on headphones.
const STREAM_KBPS: u32 = 192;
const OPUS_STREAM_KBPS: u32 = 128;

fn source_err(error: impl std::fmt::Display) -> unsub_subsonic::Error {
    unsub_subsonic::Error::Source(error.to_string())
}

// Library album/artist keys contain control characters, so they travel hex-encoded: reversible
// without a lookup table, and stable across restarts so phone apps' caches stay valid.
fn encode(prefix: &str, key: &str) -> String {
    let hex: String = key.bytes().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}{hex}")
}

fn decode(prefix: &str, id: &str) -> Option<String> {
    let hex = id.strip_prefix(prefix)?;
    let bytes: Option<Vec<u8>> =
        (0..hex.len()).step_by(2).map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect();
    String::from_utf8(bytes?).ok()
}

fn track_id(id: &str) -> Option<i64> {
    id.strip_prefix("tr")?.parse().ok()
}

fn content_type(codec: &str) -> &'static str {
    match codec {
        "FLAC" => "audio/flac",
        "MP3" => "audio/mpeg",
        "AAC" | "ALAC" => "audio/mp4",
        "Vorbis" | "Opus" => "audio/ogg",
        "WAV" => "audio/wav",
        "AIFF" => "audio/aiff",
        _ => "application/octet-stream",
    }
}

/// What the Music source needs from a song's album to link it back (Subsonic's albumId).
fn album_key_for(albums: &[AlbumSummary], track: &Track) -> Option<String> {
    let artist = if track.album_artist.is_empty() { &track.artist } else { &track.album_artist };
    albums
        .iter()
        .find(|a| a.album.eq_ignore_ascii_case(&track.album) && (a.compilation || a.album_artist.eq_ignore_ascii_case(artist)))
        .map(|a| a.key.clone())
}

fn song(track: &Track, album_key: Option<&str>) -> Song {
    let path = Path::new(&track.path);
    Song {
        id: format!("tr{}", track.id),
        parent: album_key.map(|k| encode("al", k)),
        title: track.title.clone(),
        album: Some(track.album.clone()).filter(|a| !a.is_empty()),
        artist: Some(track.artist.clone()).filter(|a| !a.is_empty()),
        track: track.track_number,
        disc_number: track.disc_number,
        year: track.year,
        genre: Some(track.genre.clone()).filter(|g| !g.is_empty()),
        size: Some(track.file_size),
        content_type: Some(content_type(&track.codec).to_owned()),
        suffix: path.extension().map(|e| e.to_string_lossy().to_lowercase()),
        duration: Some((track.duration_ms / 1000) as u32),
        bit_rate: track.bitrate_kbps,
        path: path.file_name().map(|n| n.to_string_lossy().into_owned()),
        album_id: album_key.map(|k| encode("al", k)),
        artist_id: None,
        cover_art: Some(format!("tr{}", track.id)),
        play_count: Some(u64::from(track.play_count)),
        created: Some(track.date_added),
        // The library doesn't record when a song got its fifth star; when it was added stands in.
        starred: (track.rating == STARRED_RATING).then_some(track.date_added),
        user_rating: Some(track.rating / 20).filter(|&stars| stars > 0),
    }
}

fn album(summary: &AlbumSummary) -> Album {
    Album {
        id: encode("al", &summary.key),
        name: summary.album.clone(),
        artist: Some(summary.album_artist.clone()),
        artist_id: None,
        year: summary.year,
        genre: Some(summary.genre.clone()).filter(|g| !g.is_empty()),
        song_count: summary.track_count,
        duration: (summary.duration_ms / 1000) as u32,
        cover_art: Some(encode("al", &summary.key)),
        created: summary.date_added,
        starred: None,
        user_rating: None,
    }
}

fn music() -> TrackQuery {
    TrackQuery { kind: Some(MediaKind::Music), ..TrackQuery::default() }
}

fn page<T>(items: Vec<T>, (count, offset): (usize, usize)) -> Vec<T> {
    items.into_iter().skip(offset).take(count).collect()
}

/// Serves the library to Subsonic clients over its own SQLite connection, so phone traffic never
/// waits on the song list (WAL lets both read at once).
struct LibrarySource {
    library: Mutex<Library>,
}

impl LibrarySource {
    fn rate_songs(&self, targets: &Targets, rating: u8) -> SourceResult<()> {
        if targets.songs.is_empty() && !(targets.albums.is_empty() && targets.artists.is_empty()) {
            return Err(unsub_subsonic::Error::Unsupported("starring albums and artists"));
        }
        let mut library = self.library();
        for id in targets.songs.iter().filter_map(|id| track_id(id)) {
            library.set_rating(id, rating).map_err(source_err)?;
        }
        Ok(())
    }

    /// A regular playlist's library id; smart playlists and folders aren't phones' to change.
    fn editable_playlist(&self, id: &str) -> SourceResult<i64> {
        let id = id.strip_prefix("pl").and_then(|n| n.parse::<i64>().ok()).ok_or(unsub_subsonic::Error::Unsupported("that playlist"))?;
        let playlists = self.library().playlists().map_err(source_err)?;
        match playlists.iter().find(|p| p.id == id).map(|p| p.kind) {
            Some(PlaylistKind::Regular) => Ok(id),
            _ => Err(unsub_subsonic::Error::Unsupported("editing smart playlists")),
        }
    }

    fn library(&self) -> MutexGuard<'_, Library> {
        self.library.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn songs(&self, tracks: &[Track]) -> SourceResult<Vec<Song>> {
        let albums = self.library().albums(&music()).map_err(source_err)?;
        Ok(tracks.iter().map(|t| song(t, album_key_for(&albums, t).as_deref())).collect())
    }
}

impl MusicSource for LibrarySource {
    fn artists(&self) -> SourceResult<Vec<Artist>> {
        let artists = self.library().artists(&music()).map_err(source_err)?;
        Ok(artists
            .into_iter()
            .map(|a| Artist { id: encode("ar", &a.key), name: a.name, album_count: a.album_count, cover_art: None, starred: None })
            .collect())
    }

    fn artist(&self, id: &str) -> SourceResult<Option<(Artist, Vec<Album>)>> {
        let Some(key) = decode("ar", id) else { return Ok(None) };
        let library = self.library();
        let Some(summary) = library.artists(&music()).map_err(source_err)?.into_iter().find(|a| a.key == key) else { return Ok(None) };
        let albums = library.artist_albums(&key).map_err(source_err)?;
        let artist = Artist { id: id.to_owned(), name: summary.name, album_count: summary.album_count, cover_art: None, starred: None };
        Ok(Some((artist, albums.iter().map(album).collect())))
    }

    fn album(&self, id: &str) -> SourceResult<Option<(Album, Vec<Song>)>> {
        let Some(key) = decode("al", id) else { return Ok(None) };
        let library = self.library();
        let Some(summary) = library.albums(&music()).map_err(source_err)?.into_iter().find(|a| a.key == key) else { return Ok(None) };
        let tracks = library.album_tracks(&key).map_err(source_err)?;
        Ok(Some((album(&summary), tracks.iter().map(|t| song(t, Some(&key))).collect())))
    }

    fn song(&self, id: &str) -> SourceResult<Option<Song>> {
        let Some(id) = track_id(id) else { return Ok(None) };
        let track = self.library().track(id).map_err(source_err)?;
        Ok(match track {
            Some(t) => self.songs(&[t])?.pop(),
            None => None,
        })
    }

    fn album_list(
        &self,
        kind: AlbumListKind,
        size: usize,
        offset: usize,
        genre: Option<&str>,
        years: Option<(i32, i32)>,
    ) -> SourceResult<Vec<Album>> {
        let library = self.library();
        let sort = match kind {
            AlbumListKind::AlphabeticalByName => SortKey::Album,
            _ => SortKey::Artist,
        };
        let mut albums = library.albums(&TrackQuery { sort, ..music() }).map_err(source_err)?;
        match kind {
            AlbumListKind::Random => fastrand::shuffle(&mut albums),
            AlbumListKind::Starred => {
                let starred = library.tracks(&music()).map_err(source_err)?;
                let keys: Vec<String> =
                    starred.iter().filter(|t| t.rating == STARRED_RATING).filter_map(|t| album_key_for(&albums, t)).collect();
                albums.retain(|a| keys.contains(&a.key));
            }
            AlbumListKind::Newest => albums.sort_by_key(|a| std::cmp::Reverse(a.date_added)),
            AlbumListKind::Frequent | AlbumListKind::Recent => {
                // Albums ordered by their most-played (or most recently played) song.
                let sort = if kind == AlbumListKind::Frequent { SortKey::Plays } else { SortKey::LastPlayed };
                let tracks = library.tracks(&TrackQuery { sort, descending: true, ..music() }).map_err(source_err)?;
                let mut keys: Vec<String> = Vec::new();
                for track in
                    tracks.iter().filter(|t| if kind == AlbumListKind::Frequent { t.play_count > 0 } else { t.last_played.is_some() })
                {
                    if let Some(key) = album_key_for(&albums, track).filter(|k| !keys.contains(k)) {
                        keys.push(key);
                    }
                }
                albums.retain(|a| keys.contains(&a.key));
                albums.sort_by_key(|a| keys.iter().position(|k| *k == a.key));
            }
            AlbumListKind::ByYear => {
                if let Some((from, to)) = years {
                    let (low, high) = (from.min(to), from.max(to));
                    albums.retain(|a| a.year.is_some_and(|y| (low..=high).contains(&y)));
                    albums.sort_by_key(|a| a.year);
                    if from > to {
                        albums.reverse();
                    }
                }
            }
            AlbumListKind::ByGenre => {
                if let Some(genre) = genre {
                    albums.retain(|a| a.genre.eq_ignore_ascii_case(genre));
                }
            }
            AlbumListKind::AlphabeticalByName | AlbumListKind::AlphabeticalByArtist => {}
        }
        Ok(page(albums, (size, offset)).iter().map(album).collect())
    }

    fn random_songs(&self, size: usize, genre: Option<&str>, years: Option<(i32, i32)>) -> SourceResult<Vec<Song>> {
        let mut tracks = self.library().tracks(&music()).map_err(source_err)?;
        tracks.retain(|t| t.enabled && genre.is_none_or(|g| t.genre.eq_ignore_ascii_case(g)));
        if let Some((from, to)) = years {
            tracks.retain(|t| t.year.is_some_and(|y| (from..=to).contains(&y)));
        }
        fastrand::shuffle(&mut tracks);
        tracks.truncate(size);
        self.songs(&tracks)
    }

    fn search(&self, query: &str, artists: (usize, usize), albums: (usize, usize), songs: (usize, usize)) -> SourceResult<SearchResult> {
        // An empty query means "everything": sync-all clients page through the library this way.
        let search = Some(query.trim().to_owned()).filter(|q| !q.is_empty());
        let query = TrackQuery { search, ..music() };
        let (found_artists, found_albums, found_tracks) = {
            let library = self.library();
            (
                library.artists(&query).map_err(source_err)?,
                library.albums(&query).map_err(source_err)?,
                library.tracks(&query).map_err(source_err)?,
            )
        };
        Ok(SearchResult {
            artists: page(found_artists, artists)
                .into_iter()
                .map(|a| Artist { id: encode("ar", &a.key), name: a.name, album_count: a.album_count, cover_art: None, starred: None })
                .collect(),
            albums: page(found_albums, albums).iter().map(album).collect(),
            songs: self.songs(&page(found_tracks, songs))?,
        })
    }

    fn genres(&self) -> SourceResult<Vec<Genre>> {
        let genres = self.library().genres(&music()).map_err(source_err)?;
        Ok(genres.into_iter().map(|g| Genre { value: g.name, song_count: g.track_count, album_count: g.album_count }).collect())
    }

    fn playlists(&self) -> SourceResult<Vec<Playlist>> {
        let library = self.library();
        let playlists = library.playlists().map_err(source_err)?;
        playlists
            .into_iter()
            .filter(|p| !matches!(p.kind, PlaylistKind::Folder))
            .map(|p| {
                let stats = library.stats(&TrackQuery { kind: None, playlist: Some(p.id), ..TrackQuery::default() }).map_err(source_err)?;
                let smart = p.kind == PlaylistKind::Smart;
                Ok(Playlist {
                    id: format!("pl{}", p.id),
                    name: p.name,
                    song_count: p.track_count,
                    duration: (stats.total_duration_ms / 1000) as u32,
                    created: 0,
                    changed: 0,
                    owner: None,
                    comment: None,
                    public: Some(false),
                    // Smart playlists follow their rules; phones may list them, not edit them.
                    readonly: Some(smart),
                })
            })
            .collect()
    }

    fn playlist(&self, id: &str) -> SourceResult<Option<(Playlist, Vec<Song>)>> {
        let Some(id) = id.strip_prefix("pl").and_then(|n| n.parse::<i64>().ok()) else { return Ok(None) };
        let Some(summary) = self.playlists()?.into_iter().find(|p| p.id == format!("pl{id}")) else { return Ok(None) };
        let tracks = self
            .library()
            .tracks(&TrackQuery { kind: None, playlist: Some(id), sort: SortKey::PlaylistOrder, ..TrackQuery::default() })
            .map_err(source_err)?;
        Ok(Some((summary, self.songs(&tracks)?)))
    }

    fn cover_art(&self, id: &str, size: Option<u32>) -> SourceResult<Option<(String, Vec<u8>)>> {
        let library = self.library();
        let track = match (track_id(id), decode("al", id)) {
            (Some(track), _) => Some(track),
            (None, Some(key)) => {
                library.albums(&music()).map_err(source_err)?.into_iter().find(|a| a.key == key).map(|a| a.artwork_track_id)
            }
            _ => None,
        };
        let Some(track) = track else { return Ok(None) };
        // Phones ask for a size; sending originals would burn their data plan on cover art.
        let art = match size {
            Some(size) => library.artwork_thumbnail(track, size),
            None => library.artwork(track),
        };
        Ok(art.map_err(source_err)?.map(|art| (art.mime, art.bytes)))
    }

    fn open_stream(&self, song_id: &str) -> SourceResult<Option<Stream>> {
        let Some(id) = track_id(song_id) else { return Ok(None) };
        let track = self.library().track(id).map_err(source_err)?;
        Ok(track.map(|t| Stream::File { content_type: content_type(&t.codec).to_owned(), path: PathBuf::from(t.path) }))
    }

    /// Converts as the phone streams, so a lossless library plays over cellular. Rates are kept to
    /// what each encoder accepts.
    fn transcoded(
        &self,
        song_id: &str,
        format: StreamFormat,
        max_kbps: Option<u32>,
    ) -> Option<SourceResult<(Box<dyn std::io::Read + Send>, &'static str)>> {
        let track = match self.library().track(track_id(song_id)?) {
            Ok(track) => track?,
            Err(error) => return Some(Err(source_err(error))),
        };
        let target = match format {
            StreamFormat::Mp3 => Target::Mp3 { kbps: max_kbps.unwrap_or(STREAM_KBPS).clamp(32, 320) },
            StreamFormat::Aac => Target::Aac { kbps: max_kbps.unwrap_or(STREAM_KBPS).clamp(32, 320) },
            StreamFormat::Opus => Target::Opus { kbps: max_kbps.unwrap_or(OPUS_STREAM_KBPS).clamp(16, 256) },
            StreamFormat::Flac => Target::Flac,
        };
        let options = TranscodeOptions { target, max_sample_rate: None, max_bit_depth: None, keep_tags: false };
        Some(unsub_playback::transcode_stream(Path::new(&track.path), &options).map_err(source_err))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { playlist_editing: true, lyrics: false }
    }

    fn starred(&self) -> SourceResult<Starred> {
        let tracks: Vec<Track> =
            self.library().tracks(&music()).map_err(source_err)?.into_iter().filter(|t| t.rating == STARRED_RATING).collect();
        Ok(Starred { artists: Vec::new(), albums: Vec::new(), songs: self.songs(&tracks)? })
    }

    fn songs_by_genre(&self, genre: &str, count: usize, offset: usize) -> SourceResult<Vec<Song>> {
        let tracks: Vec<Track> =
            self.library().tracks(&music()).map_err(source_err)?.into_iter().filter(|t| t.genre.eq_ignore_ascii_case(genre)).collect();
        self.songs(&page(tracks, (count, offset)))
    }

    /// Phones rate songs only; albums and artists have no rating of their own here.
    fn set_rating(&self, id: &str, stars: u8) -> SourceResult<()> {
        let id = track_id(id).ok_or(unsub_subsonic::Error::Unsupported("rating albums and artists"))?;
        self.library().set_rating(id, stars.min(5) * 20).map_err(source_err)
    }

    fn star(&self, targets: &Targets) -> SourceResult<()> {
        self.rate_songs(targets, STARRED_RATING)
    }

    fn unstar(&self, targets: &Targets) -> SourceResult<()> {
        self.rate_songs(targets, 0)
    }

    fn create_playlist(&self, name: &str, song_ids: &[String]) -> SourceResult<String> {
        let mut library = self.library();
        let playlist = library.create_playlist(name, PlaylistKind::Regular, None).map_err(source_err)?;
        let ids: Vec<i64> = song_ids.iter().filter_map(|id| track_id(id)).collect();
        library.add_to_playlist(playlist.id, &ids, None).map_err(source_err)?;
        Ok(format!("pl{}", playlist.id))
    }

    fn update_playlist(&self, id: &str, edit: &PlaylistEdit) -> SourceResult<()> {
        let id = self.editable_playlist(id)?;
        let mut library = self.library();
        if let Some(name) = &edit.name {
            library.rename_playlist(id, name).map_err(source_err)?;
        }
        if !edit.remove.is_empty() {
            let positions: Vec<u32> = edit.remove.iter().filter_map(|&p| u32::try_from(p).ok()).collect();
            library.remove_from_playlist(id, &positions).map_err(source_err)?;
        }
        let ids: Vec<i64> = edit.add.iter().filter_map(|song| track_id(song)).collect();
        if !ids.is_empty() {
            library.add_to_playlist(id, &ids, None).map_err(source_err)?;
        }
        Ok(())
    }

    fn delete_playlist(&self, id: &str) -> SourceResult<()> {
        let id = self.editable_playlist(id)?;
        self.library().delete_playlist(id).map_err(source_err)
    }

    /// Phones report finished songs; each counts as a play, as on the Mac.
    fn scrobble(&self, song_id: &str, time_ms: i64) -> SourceResult<()> {
        let Some(id) = track_id(song_id) else { return Ok(()) };
        self.library().record_play(id, time_ms / 1000).map_err(source_err)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharingSettings {
    pub enabled: bool,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// "Stream at most N kbps" for phones that don't ask for a rate; `None` sends originals.
    /// Defaulted so settings saved before it existed still load.
    #[serde(default)]
    pub max_kbps: Option<u32>,
    /// What other Unsub Macs see under SHARED, as iTunes' "Shared name".
    #[serde(default = "default_shared_name")]
    pub shared_name: String,
}

fn default_shared_name() -> String {
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    let mut letters = user.chars();
    match letters.next() {
        Some(first) => format!("{}{}'s Library", first.to_uppercase(), letters.as_str()),
        None => "Unsub Library".to_owned(),
    }
}

impl Default for SharingSettings {
    /// Off until the user turns it on; 4533 is Navidrome's port, which many apps suggest by default.
    fn default() -> Self {
        let password: String = std::iter::repeat_with(fastrand::alphanumeric).take(12).collect();
        Self { enabled: false, port: 4533, username: "unsub".to_owned(), password, max_kbps: None, shared_name: default_shared_name() }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharingStatus {
    settings: SharingSettings,
    running: bool,
    addresses: Vec<String>,
    error: Option<String>,
}

pub struct SharingState {
    settings_path: PathBuf,
    db_path: PathBuf,
    artwork_cache: PathBuf,
    settings: Mutex<SharingSettings>,
    server: Mutex<Option<Server>>,
    /// Announces the library to other Unsub Macs while the server runs.
    advertisement: Mutex<Option<unsub_remote::Advertisement>>,
    last_error: Mutex<Option<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl SharingState {
    pub fn load(settings_path: PathBuf, db_path: PathBuf, artwork_cache: PathBuf) -> Self {
        let settings = std::fs::read(&settings_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let state = Self {
            settings_path,
            db_path,
            artwork_cache,
            settings: Mutex::new(settings),
            server: Mutex::new(None),
            advertisement: Mutex::new(None),
            last_error: Mutex::new(None),
        };
        state.apply();
        state
    }

    /// Starts, restarts or stops the server to match the saved settings.
    fn apply(&self) {
        *lock(&self.advertisement) = None;
        if let Some(server) = lock(&self.server).take() {
            server.stop();
        }
        let settings = lock(&self.settings).clone();
        if !settings.enabled {
            *lock(&self.last_error) = None;
            return;
        }
        let started = Library::open(&self.db_path).map_err(|e| e.to_string()).and_then(|mut library| {
            library.set_artwork_cache(self.artwork_cache.clone());
            let config = ServerConfig {
                bind: SocketAddr::from((Ipv4Addr::UNSPECIFIED, settings.port)),
                username: settings.username.clone(),
                password: settings.password.clone(),
                api_key: None,
                default_max_kbps: settings.max_kbps,
            };
            Server::start(config, Arc::new(LibrarySource { library: Mutex::new(library) })).map_err(|e| e.to_string())
        });
        match started {
            Ok(server) => {
                *lock(&self.server) = Some(server);
                *lock(&self.last_error) = None;
                // Without it the library still works for phones; other Macs just won't list it.
                *lock(&self.advertisement) = unsub_remote::advertise(&settings.shared_name, settings.port)
                    .map_err(|error| eprintln!("[sharing] can't announce the library: {error}"))
                    .ok();
            }
            Err(error) => *lock(&self.last_error) = Some(error),
        }
    }

    /// The name this library is announced under, while it is.
    pub fn shared_name(&self) -> Option<String> {
        lock(&self.advertisement).as_ref().map(|_| lock(&self.settings).shared_name.clone())
    }

    fn status(&self) -> SharingStatus {
        let settings = lock(&self.settings).clone();
        let running = lock(&self.server).is_some();
        // Phone apps need this Mac's LAN address, not localhost.
        let addresses = if running {
            if_addrs::get_if_addrs()
                .unwrap_or_default()
                .into_iter()
                .filter(|i| !i.is_loopback() && i.ip().is_ipv4())
                .map(|i| format!("http://{}:{}", i.ip(), settings.port))
                .collect()
        } else {
            Vec::new()
        };
        SharingStatus { settings, running, addresses, error: lock(&self.last_error).clone() }
    }
}

#[tauri::command]
pub fn sharing_status(state: State<'_, SharingState>) -> SharingStatus {
    state.status()
}

#[tauri::command]
pub async fn sharing_update(state: State<'_, SharingState>, settings: SharingSettings) -> CommandResult<SharingStatus> {
    if settings.username.trim().is_empty() || settings.password.is_empty() {
        return Err(CommandError::from("A user name and password are required so only your devices can connect.".to_owned()));
    }
    let json = serde_json::to_vec(&settings).map_err(|e| CommandError::from(e.to_string()))?;
    std::fs::write(&state.settings_path, json).map_err(|e| CommandError::from(e.to_string()))?;
    *lock(&state.settings) = settings;
    state.apply();
    Ok(state.status())
}

#[cfg(test)]
mod tests {
    use super::{LibrarySource, decode, encode, track_id};
    use crate::test_support::track;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use unsub_library::{Library, MediaKind, Track};
    use unsub_subsonic::{Server, ServerConfig};

    /// What another Unsub Mac does with this one's shared library, end to end through
    /// the real pieces: this library's source behind the Subsonic server, and the remote client
    /// signing in, paging every song and fetching one byte for byte.
    #[test]
    fn another_mac_can_sign_in_list_and_fetch_this_library() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/playback/tests/fixtures");
        let song = |id: i64, file: &str, codec: &str| Track {
            path: fixtures.join(file).to_string_lossy().into_owned(),
            file_size: std::fs::metadata(fixtures.join(file)).unwrap().len(),
            title: format!("Tone {id}"),
            ..track(id, codec, MediaKind::Music)
        };
        let mut library = Library::open_in_memory().unwrap();
        library.import_tracks(&[song(1, "sine.flac", "FLAC"), song(2, "sine.mp3", "MP3"), song(3, "sine-aac.m4a", "AAC")]).unwrap();
        let config = ServerConfig {
            bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            username: "unsub".into(),
            password: "k3Xq9vR2".into(),
            api_key: None,
            default_max_kbps: None,
        };
        let server = Server::start(config, Arc::new(LibrarySource { library: Mutex::new(library) })).unwrap();
        let url = format!("http://{}", server.local_addr());

        assert!(unsub_remote::Client::new(&url, "unsub", "wrong").ping().is_err());
        let client = unsub_remote::Client::new(&url, "unsub", "k3Xq9vR2");
        client.ping().unwrap();

        // One song per page, the way the app pages: until an empty page comes back.
        let mut songs = Vec::new();
        loop {
            let page = client.songs(u32::try_from(songs.len()).unwrap(), 1).unwrap();
            if page.is_empty() {
                break;
            }
            songs.extend(page);
        }
        let mut titles: Vec<&str> = songs.iter().map(|s| s.title.as_str()).collect();
        titles.sort_unstable();
        assert_eq!(titles, ["Tone 1", "Tone 2", "Tone 3"]);

        let cache = tempfile::tempdir().unwrap();
        let flac = songs.iter().find(|s| s.suffix.as_deref() == Some("flac")).unwrap();
        let fetched = client.download_to_cache(&flac.id, cache.path()).unwrap();
        assert_eq!(std::fs::read(fetched).unwrap(), std::fs::read(fixtures.join("sine.flac")).unwrap());
        server.stop();
    }

    #[test]
    fn ids_round_trip_including_control_characters() {
        let key = "the beatles\u{1f}abbey road";
        assert_eq!(decode("al", &encode("al", key)).as_deref(), Some(key));
        assert_eq!(decode("ar", &encode("al", key)), None);
        assert_eq!(decode("al", "alzz"), None);
        assert_eq!(track_id("tr42"), Some(42));
        assert_eq!(track_id("pl42"), None);
    }
}
