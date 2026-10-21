use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use unsub_library::{MediaKind, SortKey, Track, TrackQuery};
use unsub_remote::{Browser, Client, Peer, Song};

use crate::error::{CommandError, CommandResult};

pub const CHANGED_EVENT: &str = "remote://changed";
/// Songs asked for per request while a shared library loads; the list fills in page by page.
const PAGE: u32 = 500;
/// Songs fetched from shared libraries are kept up to this size, least recently used going first.
const CACHE_BYTES: u64 = 4 << 30;
/// A server may list as many songs as it likes, and each one is held in memory.
const MAX_SONGS: usize = 500_000;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn to_error(error: impl std::fmt::Display) -> CommandError {
    CommandError::from(error.to_string())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Login {
    url: String,
    username: String,
    password: String,
}

/// What Remote.json keeps: servers added by hand, and the login for each library by its id
/// ("peer:<name>" for Macs found on the network, "server:<url>" for servers added by hand).
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Saved {
    servers: Vec<SavedServer>,
    logins: HashMap<String, Login>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedServer {
    name: String,
    url: String,
}

struct Connected {
    client: Arc<Client>,
    /// Track ids are positions in this list plus one, so the song list and the player can deal
    /// in numbers like everywhere else.
    songs: Vec<Song>,
    loading: bool,
}

/// iTunes' SHARED section: other Unsub libraries on the network and Subsonic servers.
pub struct RemoteState {
    path: PathBuf,
    cache_dir: PathBuf,
    saved: Mutex<Saved>,
    peers: Mutex<Vec<Peer>>,
    browser: Mutex<Option<Browser>>,
    connected: Mutex<HashMap<String, Connected>>,
}

impl RemoteState {
    pub fn load(path: PathBuf, cache_dir: PathBuf) -> Self {
        let saved = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self {
            path,
            cache_dir,
            saved: Mutex::new(saved),
            peers: Mutex::new(Vec::new()),
            browser: Mutex::new(None),
            connected: Mutex::new(HashMap::new()),
        }
    }

    fn save(&self, saved: &Saved) -> CommandResult<()> {
        let json = serde_json::to_vec_pretty(saved).map_err(to_error)?;
        // Other people's passwords.
        crate::write_private(&self.path, &json).map_err(to_error)
    }

    /// What the player needs to fetch a shared song: the client, the server's own id for the
    /// song, its library's name for the LCD, and where the cache lives.
    pub(crate) fn song(&self, library: &str, track_id: i64) -> CommandResult<(Arc<Client>, String, String, PathBuf)> {
        let connected = lock(&self.connected);
        let library_state = connected.get(library).ok_or_else(|| to_error("that shared library is no longer connected"))?;
        let index = usize::try_from(track_id - 1).map_err(to_error)?;
        let song = library_state.songs.get(index).ok_or_else(|| to_error("that song is no longer in the shared library"))?;
        Ok((Arc::clone(&library_state.client), song.id.clone(), self.name_of(library), self.cache_dir.clone()))
    }

    /// What the shared library says of one of its songs.
    pub(crate) fn song_info(&self, library: &str, track_id: i64) -> Option<Song> {
        let connected = lock(&self.connected);
        let index = usize::try_from(track_id - 1).ok()?;
        connected.get(library)?.songs.get(index).cloned()
    }

    /// Tells the library's owner the song was played, as a phone app would.
    pub(crate) fn scrobble(&self, library: &str, track_id: i64) {
        if let Ok((client, song_id, _, _)) = self.song(library, track_id) {
            std::thread::spawn(move || {
                let _ = client.scrobble(&song_id, true);
            });
        }
    }

    fn name_of(&self, library: &str) -> String {
        if let Some(name) = library.strip_prefix("peer:") {
            return name.to_owned();
        }
        let url = library.strip_prefix("server:").unwrap_or(library);
        lock(&self.saved).servers.iter().find(|s| s.url == url).map_or_else(|| url.to_owned(), |s| s.name.clone())
    }
}

/// Looks for other Unsub libraries for the life of the app, leaving out this Mac's own.
pub fn browse(app: &AppHandle) {
    let handle = app.clone();
    let browser = unsub_remote::browse(move |peers| {
        let own = handle.state::<crate::sharing::SharingState>().shared_name();
        *lock(&handle.state::<RemoteState>().peers) = peers.into_iter().filter(|p| own.as_deref() != Some(p.name.as_str())).collect();
        let _ = handle.emit(CHANGED_EVENT, ());
    });
    match browser {
        Ok(browser) => *lock(&app.state::<RemoteState>().browser) = Some(browser),
        Err(error) => eprintln!("[remote] can't look for shared libraries: {error}"),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedLibrary {
    id: String,
    name: String,
    /// Found on the network, or added by hand.
    discovered: bool,
    /// A login is saved and the library is where it was saved for.
    has_login: bool,
    /// A login is saved, but the library answers to its name from somewhere else now.
    address_changed: bool,
    connected: bool,
    loading: bool,
    song_count: usize,
}

#[tauri::command]
pub fn remote_libraries(state: State<'_, RemoteState>) -> Vec<SharedLibrary> {
    let saved = lock(&state.saved);
    let connected = lock(&state.connected);
    let entry = |id: String, name: String, discovered: bool, address_changed: bool| {
        let live = connected.get(&id);
        SharedLibrary {
            has_login: saved.logins.contains_key(&id) && !address_changed,
            address_changed,
            connected: live.is_some(),
            loading: live.is_some_and(|c| c.loading),
            song_count: live.map_or(0, |c| c.songs.len()),
            id,
            name,
            discovered,
        }
    };
    let peers = lock(&state.peers);
    peers
        .iter()
        .map(|p| {
            let id = format!("peer:{}", p.name);
            let address_changed = saved.logins.get(&id).is_some_and(|login| !at_saved_address(p, login));
            entry(id, p.name.clone(), true, address_changed)
        })
        .chain(saved.servers.iter().map(|s| entry(format!("server:{}", s.url), s.name.clone(), false, false)))
        .collect()
}

fn peer_named(state: &RemoteState, library: &str) -> Option<Peer> {
    let name = library.strip_prefix("peer:")?;
    lock(&state.peers).iter().find(|p| p.name == name).cloned()
}

fn peer_url(state: &RemoteState, library: &str) -> Option<String> {
    let peer = peer_named(state, library)?;
    // IPv4 first: a link-local IPv6 address needs its interface, which isn't recorded.
    let host = peer.addresses.first().map_or_else(|| peer.host.clone(), |address| address.to_string());
    Some(format!("http://{host}:{}", peer.port))
}

/// Whether a Mac announcing a name is where the login saved under that name was made. Anyone on the network can announce any
/// name, so a saved password only goes to the host and port it was first given to.
fn at_saved_address(peer: &Peer, login: &Login) -> bool {
    let Some((host, port)) = login.url.strip_prefix("http://").and_then(|rest| rest.rsplit_once(':')) else { return false };
    let same_host = if peer.addresses.is_empty() { peer.host == host } else { peer.addresses.iter().any(|a| a.to_string() == host) };
    same_host && port == peer.port.to_string()
}

/// Logs in to a shared library (saving the login for next time) and loads its songs in the
/// background, page by page. `url` is given for a server being added by hand.
#[tauri::command]
pub async fn remote_connect(
    app: AppHandle,
    library: Option<String>,
    url: Option<String>,
    name: Option<String>,
    username: String,
    password: String,
) -> CommandResult<String> {
    let state = app.state::<RemoteState>();
    let (library, url) = match (library, url) {
        (Some(library), _) => {
            let url = peer_url(&state, &library)
                .or_else(|| library.strip_prefix("server:").map(str::to_owned))
                .ok_or_else(|| to_error("that shared library is no longer on the network"))?;
            (library, url)
        }
        (None, Some(url)) => (format!("server:{}", url.trim().trim_end_matches('/')), url.trim().trim_end_matches('/').to_owned()),
        (None, None) => return Err(to_error("a server address is needed")),
    };
    let client = Client::new(&url, &username, &password).with_cache_limit(CACHE_BYTES);
    client.ping().map_err(to_error)?;
    {
        let mut saved = lock(&state.saved);
        saved.logins.insert(library.clone(), Login { url: url.clone(), username, password });
        if library.starts_with("server:") && !saved.servers.iter().any(|s| s.url == url) {
            saved.servers.push(SavedServer { name: name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| url.clone()), url });
        }
        state.save(&saved)?;
    }
    start_loading(&app, &library, client);
    Ok(library)
}

/// Opens a library with its saved login; the UI calls this when a shared library is selected.
#[tauri::command]
pub fn remote_open(app: AppHandle, state: State<'_, RemoteState>, library: String) -> CommandResult<()> {
    if lock(&state.connected).contains_key(&library) {
        return Ok(());
    }
    let login = lock(&state.saved).logins.get(&library).cloned().ok_or_else(|| to_error("no login saved"))?;
    if peer_named(&state, &library).is_some_and(|peer| !at_saved_address(&peer, &login)) {
        return Err(to_error("This library is at a different address than before."));
    }
    start_loading(&app, &library, Client::new(&login.url, &login.username, &login.password).with_cache_limit(CACHE_BYTES));
    Ok(())
}

fn start_loading(app: &AppHandle, library: &str, client: Client) {
    let client = Arc::new(client);
    lock(&app.state::<RemoteState>().connected)
        .insert(library.to_owned(), Connected { client: Arc::clone(&client), songs: Vec::new(), loading: true });
    let _ = app.emit(CHANGED_EVENT, ());
    let (app, library) = (app.clone(), library.to_owned());
    std::thread::spawn(move || {
        let mut offset: u32 = 0;
        let outcome = loop {
            match client.songs(offset, PAGE) {
                // Servers may send fewer than asked for, so only an empty page means the end.
                Ok(page) => {
                    let done = page.is_empty();
                    offset += u32::try_from(page.len()).unwrap_or(u32::MAX);
                    let capped =
                        lock(&app.state::<RemoteState>().connected).get_mut(&library).is_some_and(|c| extend_capped(&mut c.songs, page));
                    let _ = app.emit(CHANGED_EVENT, ());
                    if done || capped {
                        break Ok(capped);
                    }
                }
                Err(error) => break Err(error),
            }
        };
        let state = app.state::<RemoteState>();
        let mut connected = lock(&state.connected);
        match outcome {
            Ok(capped) => {
                if let Some(c) = connected.get_mut(&library) {
                    c.loading = false;
                }
                if capped {
                    drop(connected);
                    let message =
                        format!("“{}” has more than {MAX_SONGS} songs; only the first {MAX_SONGS} are listed.", state.name_of(&library));
                    let _ = app.emit(crate::playback::PLAYER_EVENT, unsub_playback::PlayerEvent::Notice { message });
                }
            }
            Err(error) => {
                connected.remove(&library);
                drop(connected);
                let message = format!("Couldn't open “{}”: {error}", state.name_of(&library));
                let _ = app.emit(crate::playback::PLAYER_EVENT, unsub_playback::PlayerEvent::Notice { message });
            }
        }
        let _ = app.emit(CHANGED_EVENT, ());
    });
}

/// Adds a page of songs, stopping at `MAX_SONGS`; true when songs were left out.
fn extend_capped<T>(songs: &mut Vec<T>, page: Vec<T>) -> bool {
    let room = MAX_SONGS.saturating_sub(songs.len());
    let capped = page.len() > room;
    songs.extend(page.into_iter().take(room));
    capped
}

#[tauri::command]
pub fn remote_forget(app: AppHandle, state: State<'_, RemoteState>, library: String) -> CommandResult<()> {
    lock(&state.connected).remove(&library);
    let mut saved = lock(&state.saved);
    saved.logins.remove(&library);
    if let Some(url) = library.strip_prefix("server:") {
        saved.servers.retain(|s| s.url != url);
    }
    state.save(&saved)?;
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(())
}

fn to_track(index: usize, song: &Song) -> Track {
    let artist = song.artist.clone().unwrap_or_default();
    Track {
        id: index as i64 + 1,
        // Shared songs have no file here until played; nothing shows or opens a path for them.
        path: String::new(),
        title: song.title.clone(),
        album_artist: artist.clone(),
        artist,
        album: song.album.clone().unwrap_or_default(),
        genre: song.genre.clone().unwrap_or_default(),
        composer: String::new(),
        year: song.year,
        track_number: song.track,
        track_count: None,
        disc_number: song.disc_number,
        disc_count: None,
        duration_ms: u64::from(song.duration.unwrap_or(0)) * 1000,
        bitrate_kbps: song.bit_rate,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        codec: song.suffix.as_deref().unwrap_or("").to_uppercase(),
        file_size: song.size.unwrap_or(0),
        date_added: 0,
        date_modified: 0,
        play_count: song.play_count.map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
        last_played: None,
        skip_count: 0,
        rating: 0,
        kind: MediaKind::Music,
        enabled: true,
        in_cloud: false,
        missing: false,
        loudness: None,
        remember_position: false,
        has_lyrics: false,
        album_group: String::new(),
        compilation: false,
        sort_title: None,
        sort_artist: None,
        sort_album: None,
        sort_album_artist: None,
    }
}

/// A shared library's songs, searched and sorted here since the server only pages them out.
#[tauri::command]
pub fn remote_tracks(state: State<'_, RemoteState>, library: String, query: TrackQuery) -> Vec<Track> {
    let connected = lock(&state.connected);
    let Some(library) = connected.get(&library) else { return Vec::new() };
    let words: Vec<String> = query.search.as_deref().unwrap_or("").to_lowercase().split_whitespace().map(str::to_owned).collect();
    let mut tracks: Vec<Track> = library
        .songs
        .iter()
        .enumerate()
        .map(|(i, song)| to_track(i, song))
        .filter(|t| {
            let text = format!("{} {} {}", t.title, t.artist, t.album).to_lowercase();
            words.iter().all(|w| text.contains(w))
        })
        .collect();
    let by_album = |a: &Track, b: &Track| {
        (a.album.to_lowercase(), a.disc_number, a.track_number).cmp(&(b.album.to_lowercase(), b.disc_number, b.track_number))
    };
    tracks.sort_by(|a, b| {
        let order = match query.sort {
            SortKey::Name => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            SortKey::Time => a.duration_ms.cmp(&b.duration_ms),
            SortKey::Album => by_album(a, b),
            SortKey::Genre => a.genre.to_lowercase().cmp(&b.genre.to_lowercase()),
            SortKey::Year => a.year.cmp(&b.year),
            SortKey::Plays => a.play_count.cmp(&b.play_count),
            SortKey::Bitrate => a.bitrate_kbps.cmp(&b.bitrate_kbps),
            SortKey::Size => a.file_size.cmp(&b.file_size),
            _ => Ordering::Equal,
        };
        // Ties fall back to iTunes' artist, album, track order.
        let order = order.then_with(|| a.artist.to_lowercase().cmp(&b.artist.to_lowercase())).then_with(|| by_album(a, b));
        if query.descending { order.reverse() } else { order }
    });
    tracks
}

#[cfg(test)]
mod tests {
    use super::{Login, MAX_SONGS, at_saved_address, extend_capped};
    use std::net::IpAddr;
    use unsub_remote::Peer;

    fn peer(addresses: &[&str], port: u16) -> Peer {
        Peer {
            name: "Riley's Library".to_owned(),
            host: "rileys-macbook.local".to_owned(),
            port,
            addresses: addresses.iter().map(|a| a.parse::<IpAddr>().unwrap()).collect(),
        }
    }

    fn login(url: &str) -> Login {
        Login { url: url.to_owned(), username: "unsub".to_owned(), password: "k3Xq9vR2pL".to_owned() }
    }

    #[test]
    fn a_saved_login_is_only_for_the_host_and_port_it_was_made_at() {
        let saved = login("http://192.168.1.20:4533");
        assert!(at_saved_address(&peer(&["192.168.1.20"], 4533), &saved));
        // A Mac with several addresses may list them in another order.
        assert!(at_saved_address(&peer(&["10.0.0.5", "192.168.1.20"], 4533), &saved));
        // Someone else announcing the same name, or the same host on another port.
        assert!(!at_saved_address(&peer(&["192.168.1.99"], 4533), &saved));
        assert!(!at_saved_address(&peer(&["192.168.1.20"], 4534), &saved));
        assert!(!at_saved_address(&peer(&["192.168.1.20"], 4533), &login("not a url")));
    }

    #[test]
    fn a_login_saved_by_host_name_needs_the_same_host_name() {
        let saved = login("http://rileys-macbook.local:4533");
        assert!(at_saved_address(&peer(&[], 4533), &saved));
        let mut other = peer(&[], 4533);
        other.host = "attacker.local".to_owned();
        assert!(!at_saved_address(&other, &saved));
    }

    #[test]
    fn a_server_cannot_list_more_than_the_cap() {
        let mut songs = vec![0u8; MAX_SONGS - 2];
        assert!(!extend_capped(&mut songs, vec![1, 1]));
        assert_eq!(songs.len(), MAX_SONGS);
        assert!(extend_capped(&mut songs, vec![2]));
        assert_eq!(songs.len(), MAX_SONGS);

        let mut songs = vec![0u8; MAX_SONGS - 1];
        assert!(extend_capped(&mut songs, vec![1, 1, 1]));
        assert_eq!((songs.len(), songs[MAX_SONGS - 1]), (MAX_SONGS, 1));
    }
}
