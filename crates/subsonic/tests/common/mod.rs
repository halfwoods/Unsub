//! A fake library, a server on an ephemeral port, and a small HTTP/XML/JSON toolkit.
#![allow(dead_code)]

use std::{
    io::{self, Cursor, Read},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use serde_json::Value;
use unsub_subsonic::Artist;
use unsub_subsonic::{
    Album, AlbumListKind, Capabilities, Error, Genre, Lyrics, MusicSource, Playlist, PlaylistEdit,
    Result, SearchResult, Server, ServerConfig, Song, Starred, Stream, StreamFormat, Targets,
};

pub const NS: &str = "http://subsonic.org/restapi";
pub const USER: &str = "alice";
pub const PASSWORD: &str = "sesame";
pub const API_KEY: &str = "k3y-for-tests";
pub const SONG_FILE_LEN: usize = 10_000;
pub const TRANSCODE_LEN: usize = 100_000;
pub const CREATED: i64 = 1_700_000_000;

/// Byte `i` of the fake song file, so tests can check any slice without keeping a copy.
pub fn file_byte(i: usize) -> u8 {
    (i % 251) as u8
}

pub fn file_bytes(range: std::ops::Range<usize>) -> Vec<u8> {
    range.map(file_byte).collect()
}

pub type AlbumListCall = (
    AlbumListKind,
    usize,
    usize,
    Option<String>,
    Option<(i32, i32)>,
);
pub type RandomSongsCall = (usize, Option<String>, Option<(i32, i32)>);
pub type SearchCall = (String, (usize, usize), (usize, usize), (usize, usize));

/// What the server asked the source for, to check that request parameters arrive intact.
#[derive(Default)]
pub struct Calls {
    pub album_list: Vec<AlbumListCall>,
    pub random_songs: Vec<RandomSongsCall>,
    pub search: Vec<SearchCall>,
    pub cover_art: Vec<(String, Option<u32>)>,
    pub open_stream: Vec<String>,
    pub transcoded: Vec<(String, StreamFormat, Option<u32>)>,
    /// `(song id, time in ms)`.
    pub scrobbles: Vec<(String, i64)>,
    /// `(song id, client)`.
    pub now_playing: Vec<(String, String)>,
    /// `(id, stars)`.
    pub ratings: Vec<(String, u8)>,
    pub stars: Vec<Targets>,
    pub unstars: Vec<Targets>,
    /// `(name, song ids)`.
    pub created: Vec<(String, Vec<String>)>,
    pub edits: Vec<(String, PlaylistEdit)>,
    pub deleted: Vec<String>,
    /// `(genre, count, offset)`.
    pub songs_by_genre: Vec<(String, usize, usize)>,
}

pub type Converted = Option<Result<(Box<dyn Read + Send>, &'static str)>>;
pub type Converter = Box<dyn Fn(&str, StreamFormat, Option<u32>) -> Converted + Send + Sync>;

/// How the fake source answers `transcoded`.
#[derive(Default)]
pub enum Transcode {
    /// The default: this source cannot convert, so the original is sent.
    #[default]
    Unsupported,
    /// Canned bytes that depend on the format and bitrate asked for.
    Canned,
    /// The converter breaks.
    Failing,
    /// Something else, such as the real thing.
    With(Converter),
}

/// The bytes `Transcode::Canned` returns, so a test can tell which conversion it got.
pub fn canned(format: StreamFormat, max_kbps: Option<u32>) -> Vec<u8> {
    let name = format.name();
    let tag = ((name.len() as u8 * 16) + name.as_bytes()[0]) ^ max_kbps.unwrap_or(0) as u8;
    (0..CANNED_LEN).map(|i| (i % 251) as u8 ^ tag).collect()
}

pub const CANNED_LEN: usize = 30_000;

pub fn canned_mime(format: StreamFormat) -> &'static str {
    match format {
        StreamFormat::Mp3 => "audio/mpeg",
        StreamFormat::Aac => "audio/aac",
        StreamFormat::Opus => "audio/ogg",
        StreamFormat::Flac => "audio/flac",
    }
}

/// A playlist and the ids of its songs.
pub type Stored = (Playlist, Vec<String>);

pub struct FakeSource {
    dir: tempfile::TempDir,
    pub calls: Mutex<Calls>,
    pub transcode: Mutex<Transcode>,
    pub playlists: Mutex<Vec<Stored>>,
    pub capabilities: Mutex<Capabilities>,
}

impl FakeSource {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("song1.mp3"), file_bytes(0..SONG_FILE_LEN)).unwrap();
        std::fs::write(dir.path().join("empty.mp3"), b"").unwrap();
        std::fs::write(dir.path().join("Ünïcode, name.mp3"), file_bytes(0..10)).unwrap();
        Self {
            dir,
            calls: Mutex::default(),
            transcode: Mutex::default(),
            playlists: Mutex::new(vec![
                (
                    Playlist {
                        id: "pl-1".into(),
                        name: "Favourites".into(),
                        created: CREATED,
                        changed: CREATED + 60,
                        owner: Some(USER.into()),
                        ..Playlist::default()
                    },
                    vec!["s-1".into(), "s-3".into()],
                ),
                (
                    Playlist {
                        id: "pl-smart".into(),
                        name: "Recently Added".into(),
                        created: CREATED,
                        changed: CREATED,
                        owner: Some(USER.into()),
                        readonly: Some(true),
                        ..Playlist::default()
                    },
                    vec!["s-2".into()],
                ),
            ]),
            capabilities: Mutex::new(Capabilities {
                playlist_editing: true,
                lyrics: true,
            }),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn artists_data() -> Vec<Artist> {
        let artist = |id: &str, name: &str, album_count| Artist {
            id: id.into(),
            name: name.into(),
            album_count,
            cover_art: Some(format!("{id}-art")),
            starred: None,
        };
        let mut artists = vec![
            artist("ar-1", "The Beatles", 2),
            artist("ar-2", "Daft Punk", 1),
            artist("ar-3", "10cc", 0),
        ];
        artists[0].starred = Some(CREATED + 5);
        artists
    }

    fn albums_data() -> Vec<Album> {
        let album =
            |id: &str, name: &str, artist: (&str, &str), year, genre: &str, song_count| Album {
                id: id.into(),
                name: name.into(),
                artist: Some(artist.1.into()),
                artist_id: Some(artist.0.into()),
                year: Some(year),
                genre: Some(genre.into()),
                song_count,
                duration: 400,
                cover_art: Some(id.into()),
                created: CREATED,
                starred: None,
                user_rating: None,
            };
        let mut albums = vec![
            album(
                "al-1",
                "Abbey Road",
                ("ar-1", "The Beatles"),
                1969,
                "Rock",
                2,
            ),
            album("al-2", "Revolver", ("ar-1", "The Beatles"), 1966, "Rock", 1),
            album(
                "al-3",
                "Discovery",
                ("ar-2", "Daft Punk"),
                2001,
                "Electronic",
                1,
            ),
        ];
        albums[0].starred = Some(CREATED + 5);
        albums[0].user_rating = Some(5);
        albums
    }

    fn songs_data() -> Vec<Song> {
        let song = |id: &str, title: &str, album: (&str, &str), track| Song {
            id: id.into(),
            parent: Some(album.0.into()),
            title: title.into(),
            album: Some(album.1.into()),
            artist: Some("The Beatles".into()),
            track: Some(track),
            disc_number: Some(1),
            year: Some(1969),
            genre: Some("Rock".into()),
            size: Some(SONG_FILE_LEN as u64),
            content_type: Some("audio/mpeg".into()),
            suffix: Some("mp3".into()),
            duration: Some(259),
            bit_rate: Some(320),
            path: Some(format!("The Beatles/{}/{title}.mp3", album.1)),
            album_id: Some(album.0.into()),
            artist_id: Some("ar-1".into()),
            cover_art: Some(album.0.into()),
            play_count: Some(7),
            created: Some(CREATED),
            starred: None,
            user_rating: None,
        };
        vec![
            Song {
                starred: Some(CREATED + 5),
                user_rating: Some(5),
                ..song("s-1", "Come Together", ("al-1", "Abbey Road"), 1)
            },
            // Its file is missing on disk.
            song("s-2", "Something", ("al-1", "Abbey Road"), 2),
            // Served from a reader, as a transcode would be.
            Song {
                artist: Some("Daft Punk".into()),
                ..song("s-3", "One More Time", ("al-3", "Discovery"), 1)
            },
            // Empty file.
            song("s-4", "Taxman", ("al-2", "Revolver"), 1),
            // Markup and a character XML cannot hold.
            song(
                "s-5",
                "Tom & Jerry <live> \"x\"\u{1}",
                ("al-2", "Revolver"),
                2,
            ),
            // Streams slowly, for the graceful-stop test.
            song("s-6", "Slow Burn", ("al-2", "Revolver"), 3),
            // Opened with a download name that needs encoding.
            song("s-7", "Unicode Name", ("al-2", "Revolver"), 4),
            // Lossless, and a low bitrate mp3: what the bitrate-driven conversions are chosen between.
            Song {
                id: "s-9".into(),
                title: "Lossless".into(),
                suffix: Some("flac".into()),
                content_type: Some("audio/flac".into()),
                bit_rate: Some(900),
                ..Song::default()
            },
            Song {
                id: "s-10".into(),
                title: "Small".into(),
                suffix: Some("mp3".into()),
                content_type: Some("audio/mpeg".into()),
                bit_rate: Some(128),
                ..Song::default()
            },
            // Nothing but the required fields.
            Song {
                id: "s-8".into(),
                title: "Bare".into(),
                ..Song::default()
            },
        ]
    }

    fn page<T: Clone>(items: &[T], (count, offset): (usize, usize)) -> Vec<T> {
        items.iter().skip(offset).take(count).cloned().collect()
    }
}

/// The playlist with its count and length worked out from the songs it holds (each lasts 259 s).
fn summary((playlist, songs): &Stored) -> Playlist {
    Playlist {
        song_count: songs.len() as u32,
        duration: songs.len() as u32 * 259,
        ..playlist.clone()
    }
}

pub struct SlowReader {
    pub chunks_left: usize,
}

impl Read for SlowReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.chunks_left == 0 {
            return Ok(0);
        }
        thread::sleep(Duration::from_millis(100));
        self.chunks_left -= 1;
        buf[..1000].fill(b'z');
        Ok(1000)
    }
}

impl MusicSource for FakeSource {
    fn artists(&self) -> Result<Vec<Artist>> {
        Ok(Self::artists_data())
    }

    fn artist(&self, id: &str) -> Result<Option<(Artist, Vec<Album>)>> {
        Ok(Self::artists_data()
            .into_iter()
            .find(|a| a.id == id)
            .map(|artist| {
                let albums = Self::albums_data()
                    .into_iter()
                    .filter(|a| a.artist_id.as_deref() == Some(id))
                    .collect();
                (artist, albums)
            }))
    }

    fn album(&self, id: &str) -> Result<Option<(Album, Vec<Song>)>> {
        Ok(Self::albums_data()
            .into_iter()
            .find(|a| a.id == id)
            .map(|album| {
                let songs = Self::songs_data()
                    .into_iter()
                    .filter(|s| s.album_id.as_deref() == Some(id))
                    .collect();
                (album, songs)
            }))
    }

    fn song(&self, id: &str) -> Result<Option<Song>> {
        match id {
            "boom" => panic!("the source blew up"),
            "broken" => Err(Error::Source("database is locked".into())),
            _ => Ok(Self::songs_data().into_iter().find(|s| s.id == id)),
        }
    }

    fn album_list(
        &self,
        kind: AlbumListKind,
        size: usize,
        offset: usize,
        genre: Option<&str>,
        years: Option<(i32, i32)>,
    ) -> Result<Vec<Album>> {
        self.calls.lock().unwrap().album_list.push((
            kind,
            size,
            offset,
            genre.map(str::to_owned),
            years,
        ));
        Ok(Self::page(&Self::albums_data(), (size, offset)))
    }

    fn random_songs(
        &self,
        size: usize,
        genre: Option<&str>,
        years: Option<(i32, i32)>,
    ) -> Result<Vec<Song>> {
        self.calls
            .lock()
            .unwrap()
            .random_songs
            .push((size, genre.map(str::to_owned), years));
        Ok(Self::page(&Self::songs_data(), (size, 0)))
    }

    fn search(
        &self,
        query: &str,
        artists: (usize, usize),
        albums: (usize, usize),
        songs: (usize, usize),
    ) -> Result<SearchResult> {
        self.calls
            .lock()
            .unwrap()
            .search
            .push((query.to_owned(), artists, albums, songs));
        if query == "broken" {
            return Err(Error::Source("database is locked".into()));
        }
        let everything = query.is_empty();
        let beatles = query.eq_ignore_ascii_case("beatles");
        if !(everything || beatles) {
            // Anything else matches song titles that contain it.
            let wanted = query.to_lowercase();
            let titled: Vec<Song> = Self::songs_data()
                .into_iter()
                .filter(|song| song.title.to_lowercase().contains(&wanted))
                .collect();
            return Ok(SearchResult {
                songs: Self::page(&titled, songs),
                ..SearchResult::default()
            });
        }
        let mut found = SearchResult {
            artists: Self::page(&Self::artists_data(), artists),
            albums: Self::page(&Self::albums_data(), albums),
            songs: Self::page(&Self::songs_data(), songs),
        };
        if beatles {
            found.artists.retain(|a| a.id == "ar-1");
            found
                .albums
                .retain(|a| a.artist_id.as_deref() == Some("ar-1"));
        }
        Ok(found)
    }

    fn genres(&self) -> Result<Vec<Genre>> {
        Ok(vec![
            Genre {
                value: "Rock".into(),
                song_count: 6,
                album_count: 2,
            },
            Genre {
                value: "R&B <soul>".into(),
                song_count: 1,
                album_count: 1,
            },
        ])
    }

    fn playlists(&self) -> Result<Vec<Playlist>> {
        Ok(self.playlists.lock().unwrap().iter().map(summary).collect())
    }

    fn playlist(&self, id: &str) -> Result<Option<(Playlist, Vec<Song>)>> {
        let store = self.playlists.lock().unwrap();
        Ok(store
            .iter()
            .find(|(playlist, _)| playlist.id == id)
            .map(|stored| {
                let songs = stored
                    .1
                    .iter()
                    .filter_map(|id| Self::songs_data().into_iter().find(|song| &song.id == id))
                    .collect();
                (summary(stored), songs)
            }))
    }

    fn cover_art(&self, id: &str, size: Option<u32>) -> Result<Option<(String, Vec<u8>)>> {
        self.calls
            .lock()
            .unwrap()
            .cover_art
            .push((id.to_owned(), size));
        Ok((id == "al-1").then(|| ("image/png".to_owned(), b"\x89PNG fake image".to_vec())))
    }

    fn open_stream(&self, song_id: &str) -> Result<Option<Stream>> {
        self.calls
            .lock()
            .unwrap()
            .open_stream
            .push(song_id.to_owned());
        let file = |name: &str| Stream::File {
            path: self.path(name),
            content_type: "audio/mpeg".into(),
        };
        Ok(match song_id {
            "s-1" | "s-9" | "s-10" => Some(file("song1.mp3")),
            "s-2" => Some(file("missing.mp3")),
            "s-3" => Some(Stream::Reader {
                reader: Box::new(Cursor::new(
                    (0..TRANSCODE_LEN).map(file_byte).collect::<Vec<_>>(),
                )),
                content_type: "audio/ogg".into(),
                length: Some(TRANSCODE_LEN as u64),
            }),
            "s-4" => Some(file("empty.mp3")),
            "s-6" => Some(Stream::Reader {
                reader: Box::new(SlowReader { chunks_left: 5 }),
                content_type: "audio/ogg".into(),
                length: Some(5000),
            }),
            "s-7" => Some(file("Ünïcode, name.mp3")),
            "broken" => return Err(Error::Source("disk on fire".into())),
            _ => None,
        })
    }

    fn transcoded(&self, song_id: &str, format: StreamFormat, max_kbps: Option<u32>) -> Converted {
        self.calls
            .lock()
            .unwrap()
            .transcoded
            .push((song_id.to_owned(), format, max_kbps));
        match &*self.transcode.lock().unwrap() {
            Transcode::Unsupported => None,
            Transcode::Canned => Some(Ok((
                Box::new(Cursor::new(canned(format, max_kbps))),
                canned_mime(format),
            ))),
            Transcode::Failing => Some(Err(Error::Source("the encoder is missing".into()))),
            Transcode::With(convert) => convert(song_id, format, max_kbps),
        }
    }

    fn scrobble(&self, song_id: &str, time_ms: i64) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .scrobbles
            .push((song_id.to_owned(), time_ms));
        Ok(())
    }

    fn now_playing(&self, song_id: &str, client: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .now_playing
            .push((song_id.to_owned(), client.to_owned()));
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        *self.capabilities.lock().unwrap()
    }

    fn starred(&self) -> Result<Starred> {
        Ok(Starred {
            artists: Self::artists_data().into_iter().take(1).collect(),
            albums: Self::albums_data().into_iter().take(1).collect(),
            songs: Self::songs_data().into_iter().take(1).collect(),
        })
    }

    fn songs_by_genre(&self, genre: &str, count: usize, offset: usize) -> Result<Vec<Song>> {
        self.calls
            .lock()
            .unwrap()
            .songs_by_genre
            .push((genre.to_owned(), count, offset));
        let matching: Vec<Song> = Self::songs_data()
            .into_iter()
            .filter(|song| song.genre.as_deref() == Some(genre))
            .collect();
        Ok(Self::page(&matching, (count, offset)))
    }

    fn lyrics(&self, song_id: &str) -> Result<Option<Lyrics>> {
        match song_id {
            "s-1" => Ok(Some(Lyrics {
                text: "Here comes the sun\r\nLittle darling\n\nIt's alright\n\n".into(),
                language: Some("eng".into()),
            })),
            "s-3" => Ok(Some(Lyrics {
                text: "Around the world".into(),
                language: None,
            })),
            "broken" => Err(Error::Source("lyrics database is locked".into())),
            _ => Ok(None),
        }
    }

    fn set_rating(&self, id: &str, stars: u8) -> Result<()> {
        if id == "nope" {
            return Err(Error::NotFound);
        }
        self.calls
            .lock()
            .unwrap()
            .ratings
            .push((id.to_owned(), stars));
        Ok(())
    }

    fn star(&self, targets: &Targets) -> Result<()> {
        if targets.songs.iter().any(|id| id == "nope") {
            return Err(Error::NotFound);
        }
        self.calls.lock().unwrap().stars.push(targets.clone());
        Ok(())
    }

    fn unstar(&self, targets: &Targets) -> Result<()> {
        self.calls.lock().unwrap().unstars.push(targets.clone());
        Ok(())
    }

    fn create_playlist(&self, name: &str, song_ids: &[String]) -> Result<String> {
        let mut store = self.playlists.lock().unwrap();
        let id = format!("pl-new-{}", store.len());
        store.push((
            Playlist {
                id: id.clone(),
                name: name.to_owned(),
                created: CREATED + 100,
                changed: CREATED + 100,
                owner: Some(USER.into()),
                ..Playlist::default()
            },
            song_ids.to_vec(),
        ));
        self.calls
            .lock()
            .unwrap()
            .created
            .push((name.to_owned(), song_ids.to_vec()));
        Ok(id)
    }

    fn update_playlist(&self, id: &str, edit: &PlaylistEdit) -> Result<()> {
        let mut store = self.playlists.lock().unwrap();
        let (playlist, songs) = store
            .iter_mut()
            .find(|(playlist, _)| playlist.id == id)
            .ok_or(Error::NotFound)?;
        if playlist.readonly == Some(true) {
            return Err(Error::Unsupported("editing smart playlists"));
        }
        if let Some(name) = &edit.name {
            playlist.name.clone_from(name);
        }
        if edit.comment.is_some() {
            playlist.comment.clone_from(&edit.comment);
        }
        if edit.public.is_some() {
            playlist.public = edit.public;
        }
        let kept: Vec<String> = songs
            .iter()
            .enumerate()
            .filter(|(index, _)| !edit.remove.contains(index))
            .map(|(_, id)| id.clone())
            .collect();
        *songs = kept;
        songs.extend(edit.add.iter().cloned());
        self.calls
            .lock()
            .unwrap()
            .edits
            .push((id.to_owned(), edit.clone()));
        Ok(())
    }

    fn delete_playlist(&self, id: &str) -> Result<()> {
        let mut store = self.playlists.lock().unwrap();
        let position = store
            .iter()
            .position(|(playlist, _)| playlist.id == id)
            .ok_or(Error::NotFound)?;
        if store[position].0.readonly == Some(true) {
            return Err(Error::Unsupported("deleting smart playlists"));
        }
        store.remove(position);
        self.calls.lock().unwrap().deleted.push(id.to_owned());
        Ok(())
    }
}

pub struct Resp {
    pub status: u16,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn new(status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8(self.body.clone()).unwrap()
    }

    /// The `subsonic-response` object of a JSON reply.
    pub fn json(&self) -> Value {
        assert!(
            self.header("content-type")
                .unwrap()
                .starts_with("application/json")
        );
        let mut value: Value = serde_json::from_slice(&self.body).unwrap();
        value["subsonic-response"].take()
    }
}

pub struct Harness {
    pub server: Option<Server>,
    pub source: Arc<FakeSource>,
    pub base: String,
    agent: ureq::Agent,
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into()
}

impl Harness {
    pub fn start() -> Self {
        Self::start_with(Some(API_KEY))
    }

    pub fn start_with(api_key: Option<&str>) -> Self {
        Self::start_with_password(api_key, PASSWORD)
    }

    pub fn start_with_password(api_key: Option<&str>, password: &str) -> Self {
        Self::start_configured(|config| {
            config.api_key = api_key.map(str::to_owned);
            config.password = password.to_owned();
        })
    }

    /// The server's default cap on streaming bitrate, and how the source answers conversions.
    pub fn start_converting(default_max_kbps: Option<u32>, transcode: Transcode) -> Self {
        let harness = Self::start_configured(|config| config.default_max_kbps = default_max_kbps);
        *harness.source.transcode.lock().unwrap() = transcode;
        harness
    }

    pub fn start_configured(edit: impl FnOnce(&mut ServerConfig)) -> Self {
        let source = Arc::new(FakeSource::new());
        let mut config = ServerConfig {
            bind: ([127, 0, 0, 1], 0).into(),
            username: USER.into(),
            password: PASSWORD.into(),
            api_key: Some(API_KEY.to_owned()),
            default_max_kbps: None,
        };
        edit(&mut config);
        let server = Server::start(config, source.clone()).unwrap();
        let base = format!("http://{}", server.local_addr());
        Self {
            server: Some(server),
            source,
            base,
            agent: agent(),
        }
    }

    /// `/rest/<endpoint>` with valid credentials and the given extra query.
    pub fn url(&self, endpoint: &str, query: &str) -> String {
        format!(
            "{}/rest/{endpoint}?u={USER}&p={PASSWORD}&v=1.16.1&c=tests&{query}",
            self.base
        )
    }

    pub fn get_url(&self, url: &str) -> Resp {
        self.send(self.agent.get(url).call())
    }

    pub fn get_with_header(&self, url: &str, name: &str, value: &str) -> Resp {
        self.send(self.agent.get(url).header(name, value).call())
    }

    /// GET an endpoint with valid credentials.
    pub fn get(&self, endpoint: &str, query: &str) -> Resp {
        self.get_url(&self.url(endpoint, query))
    }

    /// GET an endpoint with valid credentials, asking for JSON.
    pub fn json(&self, endpoint: &str, query: &str) -> Value {
        let reply = self.get(endpoint, &format!("{query}&f=json"));
        assert_eq!(reply.status, 200);
        let body = reply.json();
        assert_eq!(body["status"], "ok", "{body}");
        body
    }

    pub fn post(&self, endpoint: &str, form: &[(&str, &str)]) -> Resp {
        self.post_url_with_form(&format!("{}/rest/{endpoint}", self.base), form)
    }

    pub fn post_url_with_form(&self, url: &str, form: &[(&str, &str)]) -> Resp {
        self.send(self.agent.post(url).send_form(form.iter().copied()))
    }

    fn send(
        &self,
        result: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Resp {
        let mut response = result.unwrap();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
            .collect();
        let status = response.status().as_u16();
        let body = response.body_mut().read_to_vec().unwrap();
        Resp {
            status,
            headers,
            body,
        }
    }

    pub fn stop(mut self) {
        self.server.take().unwrap().stop();
    }
}

/// Parses a reply as XML, checking the envelope and that every element is in the Subsonic namespace.
pub fn parse_xml(reply: &Resp) -> roxmltree::Document<'static> {
    assert!(
        reply
            .header("content-type")
            .unwrap()
            .starts_with("text/xml"),
        "{:?}",
        reply.header("content-type")
    );
    // Leaked so the document can outlive the reply; fine for a short-lived test process.
    let text: &'static str = Box::leak(reply.text().into_boxed_str());
    let doc = roxmltree::Document::parse(text).unwrap();
    let root = doc.root_element();
    assert_eq!(root.tag_name().name(), "subsonic-response");
    for node in doc.descendants().filter(|n| n.is_element()) {
        assert_eq!(
            node.tag_name().namespace(),
            Some(NS),
            "{}",
            node.tag_name().name()
        );
    }
    doc
}

pub fn xml_ok(reply: &Resp) -> roxmltree::Document<'static> {
    let doc = parse_xml(reply);
    assert_eq!(
        doc.root_element().attribute("status"),
        Some("ok"),
        "{}",
        reply.text()
    );
    doc
}

pub fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> roxmltree::Node<'a, 'i> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
        .unwrap_or_else(|| panic!("no <{name}> in <{}>", node.tag_name().name()))
}

pub fn children<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Vec<roxmltree::Node<'a, 'i>> {
    node.children()
        .filter(|c| c.is_element() && c.tag_name().name() == name)
        .collect()
}

/// The error code and message of a failed reply, in whichever format it came in.
pub fn error_of(reply: &Resp) -> (u32, String) {
    if reply
        .header("content-type")
        .unwrap()
        .starts_with("application/json")
    {
        let body = reply.json();
        assert_eq!(body["status"], "failed");
        (
            body["error"]["code"].as_u64().unwrap() as u32,
            body["error"]["message"].as_str().unwrap().to_owned(),
        )
    } else {
        let doc = parse_xml(reply);
        assert_eq!(doc.root_element().attribute("status"), Some("failed"));
        let error = child(doc.root_element(), "error");
        (
            error.attribute("code").unwrap().parse().unwrap(),
            error.attribute("message").unwrap().to_owned(),
        )
    }
}
