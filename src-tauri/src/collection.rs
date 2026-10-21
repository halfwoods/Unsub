use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use unsub_applemusic::{MediaType, MusicLibraryItem, Ownership};
use unsub_library::{ExternalTrack, MatchQuality, album_key, is_generic_title, song_key};

use crate::error::{CommandError, CommandResult};
use crate::library::{self, LibraryState};

pub const CHANGED_EVENT: &str = "collection://changed";

/// The last Apple Music library we read. Kept on disk so the comparison survives restarts and
/// re-runs against the library as new purchases are imported.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    source: String,
    loaded_at: i64,
    items: Vec<MusicLibraryItem>,
}

/// Where a streaming export found a song.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub service: String,
    /// As the list says it: `Spotify playlist “Road Trip”`.
    pub label: String,
}

/// A song someone streams that wasn't in their library when a streaming service's export was
/// imported. It is listed beside the Apple Music songs and drops off once the library has it.
/// One song is one entry, however many playlists and services named it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamedSong {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: Option<u64>,
    pub sources: Vec<Source>,
}

impl StreamedSong {
    /// Stands for the entry in the UI, which asks to remove it. Never two entries alike: an album
    /// and no album are one entry, and two albums are two.
    fn id(&self) -> String {
        format!("streamed:{}\u{1f}{}\u{1f}{}", self.title.to_lowercase(), self.artist.to_lowercase(), self.album.to_lowercase())
    }

    fn from(&self) -> String {
        self.sources.iter().map(|source| source.label.as_str()).collect::<Vec<_>>().join(" · ")
    }
}

pub struct CollectionState {
    path: PathBuf,
    snapshot: Mutex<Option<Snapshot>>,
    streamed_path: PathBuf,
    streamed: Mutex<Vec<StreamedSong>>,
}

impl CollectionState {
    pub fn load(path: PathBuf, streamed_path: PathBuf) -> Self {
        let snapshot = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let streamed = std::fs::read(&streamed_path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { path, snapshot: Mutex::new(snapshot), streamed_path, streamed: Mutex::new(streamed) }
    }

    fn snapshot(&self) -> MutexGuard<'_, Option<Snapshot>> {
        self.snapshot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn streamed(&self) -> MutexGuard<'_, Vec<StreamedSong>> {
        self.streamed.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save_streamed(&self, songs: &[StreamedSong]) -> CommandResult<()> {
        let json = serde_json::to_vec(songs).map_err(|e| CommandError::from(e.to_string()))?;
        std::fs::write(&self.streamed_path, json).map_err(|e| CommandError::from(e.to_string()))
    }

    /// Lists songs a streaming export had no match for, and says how many were new. A song already
    /// listed only gains where it was found: by another playlist, or another service. Nothing is
    /// removed, so importing a second export doesn't lose what the first one found; a song leaves
    /// by being found in the library, or by the person removing it. A listening history gives no
    /// album, so its song is the one a playlist listed with one, and a song first listed with no
    /// album takes the album when a playlist gives it.
    pub fn add_streamed(&self, songs: Vec<StreamedSong>) -> CommandResult<usize> {
        let mut streamed = self.streamed();
        let mut all = streamed.clone();
        let titled = |song: &StreamedSong| (song.title.to_lowercase(), song.artist.to_lowercase());
        let mut by_title: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (index, song) in all.iter().enumerate() {
            by_title.entry(titled(song)).or_default().push(index);
        }
        let (mut added, mut changed) = (0, false);
        for song in songs {
            let same_title = by_title.entry(titled(&song)).or_default();
            let album = song.album.to_lowercase();
            let found = same_title
                .iter()
                .copied()
                .find(|index| album.is_empty() || all[*index].album.to_lowercase() == album)
                .or_else(|| same_title.iter().copied().find(|index| all[*index].album.is_empty()));
            let Some(index) = found else {
                same_title.push(all.len());
                all.push(song);
                added += 1;
                changed = true;
                continue;
            };
            let known = &mut all[index];
            if known.album.is_empty() && !song.album.is_empty() {
                known.album = song.album.clone();
                changed = true;
            }
            for source in song.sources {
                if !known.sources.contains(&source) {
                    known.sources.push(source);
                    changed = true;
                }
            }
        }
        if changed {
            self.save_streamed(&all)?;
            *streamed = all;
        }
        Ok(added)
    }

    /// Takes one entry off the list, for a song the person doesn't want or that was never theirs.
    pub fn remove_streamed(&self, id: &str) -> CommandResult<()> {
        let mut streamed = self.streamed();
        let all: Vec<StreamedSong> = streamed.iter().filter(|song| song.id() != id).cloned().collect();
        self.save_streamed(&all)?;
        *streamed = all;
        Ok(())
    }

    /// Takes off everything `service` found. A song another service also found stays, with that
    /// service's place only. Says how many entries went.
    pub fn forget_streamed(&self, service: &str) -> CommandResult<usize> {
        let mut streamed = self.streamed();
        let all: Vec<StreamedSong> = streamed
            .iter()
            .cloned()
            .map(|mut song| {
                song.sources.retain(|source| source.service != service);
                song
            })
            .filter(|song| !song.sources.is_empty())
            .collect();
        let gone = streamed.len() - all.len();
        self.save_streamed(&all)?;
        *streamed = all;
        Ok(gone)
    }

    fn replace(&self, source: &str, items: Vec<MusicLibraryItem>) -> CommandResult<()> {
        let loaded_at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let snapshot = Snapshot { source: source.to_owned(), loaded_at, items };
        let json = serde_json::to_vec(&snapshot).map_err(|e| CommandError::from(e.to_string()))?;
        std::fs::write(&self.path, json).map_err(|e| CommandError::from(e.to_string()))?;
        *self.snapshot() = Some(snapshot);
        Ok(())
    }
}

/// What it takes to own each song, from nothing to do through to buying it.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum OwnStatus {
    InLibrary,
    Import,
    /// You own it, but the file the Music library points to isn't on this computer.
    Locate,
    Redownload,
    Buy,
    Unknown,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReportTrack {
    persistent_id: String,
    title: String,
    artist: String,
    duration_ms: Option<u64>,
    track_number: Option<u32>,
    disc_number: Option<u32>,
    kind: String,
    status: OwnStatus,
    track_id: Option<i64>,
    match_quality: Option<MatchQuality>,
    /// Where a streaming export found it; Apple Music songs have none.
    from: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlbumGroup {
    album_artist: String,
    album: String,
    tracks: Vec<ReportTrack>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Totals {
    songs: usize,
    in_library: usize,
    import: usize,
    locate: usize,
    redownload: usize,
    buy: usize,
    unknown: usize,
}

/// Where a batch of missing files was supposed to be, so the UI can say "connect EXTERNALNEW".
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MissingRoot {
    root: String,
    count: usize,
    /// True for a drive that's plugged in again or a folder that exists, i.e. worth a rescan.
    available: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipReport {
    source: String,
    loaded_at: i64,
    totals: Totals,
    missing_roots: Vec<MissingRoot>,
    albums: Vec<AlbumGroup>,
    /// How many of the entries each streaming service found, for forgetting its songs.
    streamed: Vec<StreamedCount>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamedCount {
    service: String,
    songs: usize,
}

/// Files come and go (drives get plugged in), so presence is checked now rather than trusted
/// from the snapshot.
fn file_here(item: &MusicLibraryItem) -> bool {
    item.location.as_deref().is_some_and(Path::exists)
}

fn status_of(item: &MusicLibraryItem, matched: bool) -> OwnStatus {
    if matched {
        return OwnStatus::InLibrary;
    }
    match item.ownership {
        Ownership::LocalFile | Ownership::MissingFile if file_here(item) => OwnStatus::Import,
        Ownership::LocalFile | Ownership::MissingFile => OwnStatus::Locate,
        Ownership::PurchasedInCloud | Ownership::UploadedInCloud => OwnStatus::Redownload,
        Ownership::Subscription => OwnStatus::Buy,
        Ownership::Unknown => OwnStatus::Unknown,
    }
}

/// Groups a missing file under the drive or folder it lived in: a whole volume, or the first few
/// folders of a path, which is where libraries keep their media.
fn root_of(path: &Path) -> PathBuf {
    let parts: Vec<Component> = path.components().collect();
    let depth = match parts.get(1) {
        Some(Component::Normal(first)) if *first == "Volumes" => 3,
        _ => 6,
    };
    parts.iter().take(depth.min(parts.len().saturating_sub(1))).collect()
}

/// Only songs and audiobooks are things you'd buy to own here; video and podcasts aren't.
fn wanted(item: &MusicLibraryItem) -> bool {
    matches!(item.media, MediaType::Music | MediaType::Audiobook)
}

pub(crate) fn external(item: &MusicLibraryItem) -> ExternalTrack {
    ExternalTrack {
        external_id: item.persistent_id.clone(),
        title: item.title.clone(),
        artist: item.artist.clone(),
        album_artist: item.album_artist.clone(),
        album: item.album.clone(),
        duration_ms: item.duration_ms,
        track_number: item.track_number,
        disc_number: item.disc_number,
        path: item.location.as_ref().map(|p| p.to_string_lossy().into_owned()),
        heard_ms: None,
    }
}

fn streamed_external(song: &StreamedSong) -> ExternalTrack {
    ExternalTrack {
        external_id: song.id(),
        title: song.title.clone(),
        artist: song.artist.clone(),
        album_artist: String::new(),
        album: song.album.clone(),
        duration_ms: song.duration_ms,
        track_number: None,
        disc_number: None,
        path: None,
        heard_ms: None,
    }
}

/// A credit is "Main, Guest, …", and an album is filed under the first.
fn primary_artist(credit: &str) -> &str {
    credit.split_once(", ").map_or(credit, |(first, _)| first)
}

fn build_report(snapshot: Option<&Snapshot>, streamed: &[StreamedSong], library: &LibraryState) -> CommandResult<OwnershipReport> {
    let items: Vec<&MusicLibraryItem> = snapshot.into_iter().flat_map(|snapshot| &snapshot.items).filter(|item| wanted(item)).collect();
    // A streamed song the Music library lists too is that row, which knows better whether it is
    // owned, and gains where the streaming export found it; it isn't a second row to buy. A title
    // as common as "Intro" is that row only on the same album.
    let mut apple_rows: HashMap<String, Vec<usize>> = HashMap::new();
    for (row, item) in items.iter().enumerate() {
        apple_rows.entry(song_key(&item.title, &item.artist)).or_default().push(row);
    }
    let mut also_from: Vec<Vec<String>> = vec![Vec::new(); items.len()];
    let mut streaming_only: Vec<&StreamedSong> = Vec::new();
    for song in streamed {
        let generic = is_generic_title(&song.title);
        let same_album = |row: &usize| album_key(&song.album).is_some_and(|album| Some(album) == album_key(&items[*row].album));
        let row = apple_rows
            .get(&song_key(&song.title, &song.artist))
            .and_then(|rows| rows.iter().copied().find(|row| !generic || same_album(row)));
        match row {
            Some(row) => also_from[row].push(song.from()),
            None => streaming_only.push(song),
        }
    }

    let externals: Vec<ExternalTrack> =
        items.iter().map(|item| external(item)).chain(streaming_only.iter().map(|song| streamed_external(song))).collect();
    let mut matches = library.queries().match_external(&externals)?;
    let streamed_matches = matches.split_off(items.len());

    let mut totals = Totals { songs: items.len(), ..Totals::default() };
    let mut missing: BTreeMap<PathBuf, usize> = BTreeMap::new();
    // Case-insensitive album keys so "The Wall" and "the wall" land together.
    let mut albums: BTreeMap<(String, String), AlbumGroup> = BTreeMap::new();
    for ((item, found), also_from) in items.into_iter().zip(matches).zip(also_from) {
        let status = status_of(item, found.is_some());
        match status {
            OwnStatus::InLibrary => totals.in_library += 1,
            OwnStatus::Import => totals.import += 1,
            OwnStatus::Locate => {
                totals.locate += 1;
                if let Some(location) = &item.location {
                    *missing.entry(root_of(location)).or_default() += 1;
                }
            }
            OwnStatus::Redownload => totals.redownload += 1,
            OwnStatus::Buy => totals.buy += 1,
            OwnStatus::Unknown => totals.unknown += 1,
        }
        let album_artist = if item.album_artist.is_empty() { item.artist.clone() } else { item.album_artist.clone() };
        let key = (album_artist.to_lowercase(), item.album.to_lowercase());
        let group = albums.entry(key).or_insert_with(|| AlbumGroup { album_artist, album: item.album.clone(), tracks: Vec::new() });
        group.tracks.push(ReportTrack {
            persistent_id: item.persistent_id.clone(),
            title: item.title.clone(),
            artist: item.artist.clone(),
            duration_ms: item.duration_ms,
            track_number: item.track_number,
            disc_number: item.disc_number,
            kind: item.kind.clone(),
            status,
            track_id: found.as_ref().map(|m| m.track_id),
            match_quality: found.map(|m| m.quality),
            from: Some(also_from.join(" · ")).filter(|from| !from.is_empty()),
        });
    }
    for (song, found) in streaming_only.into_iter().zip(streamed_matches) {
        totals.songs += 1;
        let status = if found.is_some() {
            totals.in_library += 1;
            OwnStatus::InLibrary
        } else {
            totals.buy += 1;
            OwnStatus::Buy
        };
        let album_artist = primary_artist(&song.artist).to_owned();
        let key = (album_artist.to_lowercase(), song.album.to_lowercase());
        let group = albums.entry(key).or_insert_with(|| AlbumGroup { album_artist, album: song.album.clone(), tracks: Vec::new() });
        group.tracks.push(ReportTrack {
            persistent_id: song.id(),
            title: song.title.clone(),
            artist: song.artist.clone(),
            duration_ms: song.duration_ms,
            track_number: None,
            disc_number: None,
            kind: song.sources.first().map_or_else(String::new, |source| source.service.clone()),
            status,
            track_id: found.as_ref().map(|m| m.track_id),
            match_quality: found.map(|m| m.quality),
            from: Some(song.from()),
        });
    }
    let albums = albums
        .into_values()
        .map(|mut group| {
            group.tracks.sort_by_key(|t| (t.disc_number.unwrap_or(1), t.track_number.unwrap_or(u32::MAX)));
            group
        })
        .collect();
    let mut missing_roots: Vec<MissingRoot> = missing
        .into_iter()
        .map(|(root, count)| MissingRoot { available: root.exists(), root: root.to_string_lossy().into_owned(), count })
        .collect();
    missing_roots.sort_by(|a, b| b.count.cmp(&a.count));
    Ok(OwnershipReport {
        source: snapshot.map_or_else(String::new, |snapshot| snapshot.source.clone()),
        loaded_at: snapshot.map_or(0, |snapshot| snapshot.loaded_at),
        totals,
        missing_roots,
        albums,
        streamed: streamed_counts(streamed),
    })
}

fn streamed_counts(streamed: &[StreamedSong]) -> Vec<StreamedCount> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for song in streamed {
        let services: std::collections::BTreeSet<&str> = song.sources.iter().map(|source| source.service.as_str()).collect();
        for service in services {
            *counts.entry(service).or_default() += 1;
        }
    }
    counts.into_iter().map(|(service, songs)| StreamedCount { service: service.to_owned(), songs }).collect()
}

#[tauri::command]
pub fn collection_report(state: State<'_, CollectionState>, library: State<'_, LibraryState>) -> CommandResult<Option<OwnershipReport>> {
    let (snapshot, streamed) = (state.snapshot(), state.streamed());
    if snapshot.is_none() && streamed.is_empty() {
        return Ok(None);
    }
    build_report(snapshot.as_ref(), &streamed, &library).map(Some)
}

/// Takes one streamed song off the list (the id a row of the report carries).
#[tauri::command]
pub fn collection_remove_streamed(app: AppHandle, state: State<'_, CollectionState>, id: String) -> CommandResult<()> {
    state.remove_streamed(&id)?;
    let _ = app.emit(CHANGED_EVENT, Option::<String>::None);
    Ok(())
}

/// Takes off everything one service's export put on the list, and says how many songs went.
#[tauri::command]
pub fn collection_forget_streamed(app: AppHandle, state: State<'_, CollectionState>, service: String) -> CommandResult<usize> {
    let gone = state.forget_streamed(&service)?;
    let _ = app.emit(CHANGED_EVENT, Option::<String>::None);
    Ok(gone)
}

fn finish_load(app: &AppHandle, source: &str, result: Result<Vec<MusicLibraryItem>, String>) {
    let outcome = result.and_then(|items| app.state::<CollectionState>().replace(source, items).map_err(|e| format!("{e:?}")));
    match outcome {
        Ok(()) => {
            let _ = app.emit(CHANGED_EVENT, Option::<String>::None);
        }
        Err(message) => {
            let _ = app.emit(CHANGED_EVENT, Some(message));
        }
    }
}

/// Reads the Music app's library through Apple's public framework; macOS asks permission first.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn collection_load_music_app(app: AppHandle) {
    // The first call blocks on the privacy prompt, so never run it on the IPC thread.
    std::thread::spawn(move || {
        let result = unsub_applemusic::read_system_library().map_err(|e| e.to_string());
        finish_load(&app, "Music", result);
    });
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn collection_load_music_app() -> CommandResult<()> {
    Err(CommandError::from("Reading the Music app directly needs macOS; choose an exported Library XML instead.".to_owned()))
}

/// Music: File › Library › Export Library…, or iTunes' "iTunes Library.xml" on Windows.
#[tauri::command]
pub fn collection_load_xml(app: AppHandle) {
    let handle = app.clone();
    app.dialog().file().set_title("Choose an Exported Library").add_filter("Library XML", &["xml"]).pick_file(move |file| {
        let Some(path) = file.and_then(|f| f.into_path().ok()) else { return };
        std::thread::spawn(move || {
            let result = unsub_applemusic::read_xml(&path).map_err(|e| e.to_string());
            finish_load(&handle, "Library XML", result);
        });
    });
}

/// Imports a folder the user points at, e.g. the drive or backup where missing files went.
/// Matching then marks those songs as in the library, whatever their paths are now.
#[tauri::command]
pub fn collection_locate_folder(app: AppHandle) {
    let handle = app.clone();
    app.dialog().file().set_title("Locate Your Music").pick_folder(move |folder| {
        if let Some(path) = folder.and_then(|f| f.into_path().ok()) {
            library::add_paths(&handle, vec![path]);
        }
    });
}

/// Asks the Music app to fetch each song (by persistent ID) from iCloud. Music downloads on its own
/// schedule; the songs then show up as owned files to import on the next Refresh.
#[cfg(target_os = "macos")]
const MUSIC_DOWNLOAD_SCRIPT: &str = r#"
on run persistentIDs
    set accepted to 0
    tell application "Music"
        set lib to library playlist 1
        repeat with pid in persistentIDs
            try
                download (first track of lib whose persistent ID is (pid as text))
                set accepted to accepted + 1
            end try
        end repeat
    end tell
    return accepted
end run
"#;

// Only macOS has a Music app to ask, so only it has the command that returns this.
#[cfg(target_os = "macos")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRequest {
    requested: usize,
    accepted: usize,
}

/// "Download from iCloud" for purchased and uploaded/matched songs. macOS asks once before
/// Unsub may control Music; subscription songs aren't downloadable and are never sent.
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn collection_download_from_icloud(app: AppHandle) -> CommandResult<DownloadRequest> {
    let ids: Vec<String> = {
        let state = app.state::<CollectionState>();
        let library = app.state::<LibraryState>();
        let snapshot = state.snapshot();
        let Some(snapshot) = snapshot.as_ref() else { return Ok(DownloadRequest { requested: 0, accepted: 0 }) };
        build_report(Some(snapshot), &[], &library)?
            .albums
            .into_iter()
            .flat_map(|album| album.tracks)
            .filter(|track| track.status == OwnStatus::Redownload)
            .map(|track| track.persistent_id)
            .collect()
    };
    let requested = ids.len();
    if requested == 0 {
        return Ok(DownloadRequest { requested, accepted: 0 });
    }
    // osascript blocks while Music works through the list, so keep it off the IPC thread.
    let output = tauri::async_runtime::spawn_blocking(move || {
        use std::io::Write;
        let mut child = std::process::Command::new("osascript")
            .arg("-")
            .args(&ids)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(MUSIC_DOWNLOAD_SCRIPT.as_bytes())?;
        }
        child.wait_with_output()
    })
    .await
    .map_err(|e| CommandError::from(e.to_string()))?
    .map_err(|e| CommandError::from(e.to_string()))?;
    if !output.status.success() {
        // Most often the Automation permission was declined in System Settings.
        return Err(CommandError::from(format!("Music didn't accept the request: {}", String::from_utf8_lossy(&output.stderr).trim())));
    }
    let accepted = String::from_utf8_lossy(&output.stdout).trim().parse().unwrap_or(0);
    Ok(DownloadRequest { requested, accepted })
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn collection_download_from_icloud() -> CommandResult<()> {
    Err(CommandError::from("Downloading from iCloud through Music needs macOS.".to_owned()))
}

/// Adds every owned file the Music app has that Unsub doesn't, in one import.
#[tauri::command]
pub fn collection_import_owned(
    app: AppHandle,
    state: State<'_, CollectionState>,
    library: State<'_, LibraryState>,
) -> CommandResult<usize> {
    let paths: Vec<PathBuf> = {
        let snapshot = state.snapshot();
        let Some(snapshot) = snapshot.as_ref() else { return Ok(0) };
        let report = build_report(Some(snapshot), &[], &library)?;
        let wanted: std::collections::HashSet<&str> = report
            .albums
            .iter()
            .flat_map(|album| &album.tracks)
            .filter(|track| track.status == OwnStatus::Import)
            .map(|track| track.persistent_id.as_str())
            .collect();
        snapshot.items.iter().filter(|item| wanted.contains(item.persistent_id.as_str())).filter_map(|item| item.location.clone()).collect()
    };
    let count = paths.len();
    if count > 0 {
        library::add_paths(&app, paths);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn source(service: &str, label: &str) -> Source {
        Source { service: service.into(), label: label.into() }
    }

    fn streamed(title: &str, album: &str, service: &str, label: &str) -> StreamedSong {
        StreamedSong {
            title: title.into(),
            artist: "Band".into(),
            album: album.into(),
            duration_ms: None,
            sources: vec![source(service, label)],
        }
    }

    fn state(dir: &TempDir) -> CollectionState {
        CollectionState::load(dir.path().join("Snapshot.json"), dir.path().join("Streamed.json"))
    }

    fn listed(state: &CollectionState) -> Vec<(String, String, String)> {
        state.streamed().iter().map(|song| (song.title.clone(), song.album.clone(), song.from())).collect()
    }

    #[test]
    fn a_song_is_listed_once_gains_where_it_was_found_and_survives_a_restart() {
        let dir = TempDir::new().unwrap();
        let state = state(&dir);

        let first = vec![
            streamed("Song", "Album", "Spotify", "Spotify playlist “A”"),
            streamed("SONG", "album", "Spotify", "Spotify playlist “B”"),
        ];
        assert_eq!(state.add_streamed(first).unwrap(), 1);
        let second = vec![
            streamed("Song", "Album", "Spotify", "Spotify playlist “A”"),
            streamed("Other", "", "Spotify", "Spotify listening history"),
        ];
        assert_eq!(state.add_streamed(second).unwrap(), 1);

        let expected = [
            ("Song".to_owned(), "Album".to_owned(), "Spotify playlist “A” · Spotify playlist “B”".to_owned()),
            ("Other".to_owned(), String::new(), "Spotify listening history".to_owned()),
        ];
        assert_eq!(listed(&state), expected);
        assert_eq!(listed(&CollectionState::load(dir.path().join("Snapshot.json"), dir.path().join("Streamed.json"))), expected);
    }

    #[test]
    fn a_song_with_no_album_and_the_same_song_with_one_are_one_entry_whichever_came_first() {
        for albumless_first in [true, false] {
            let dir = TempDir::new().unwrap();
            let state = state(&dir);
            let with = streamed("Song", "Album", "Spotify", "Spotify playlist “A”");
            let without = streamed("Song", "", "ListenBrainz", "ListenBrainz listening history");

            let added = if albumless_first {
                state.add_streamed(vec![without, with]).unwrap()
            } else {
                state.add_streamed(vec![with, without]).unwrap()
            };

            assert_eq!(added, 1, "albumless first: {albumless_first}");
            let rows = listed(&state);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, "Album");
        }
    }

    #[test]
    fn two_albums_are_two_entries_and_a_service_that_found_both_is_counted_once_per_entry() {
        let dir = TempDir::new().unwrap();
        let state = state(&dir);

        let both = vec![streamed("Song", "Album", "Spotify", "x"), streamed("Song", "Greatest Hits", "Spotify", "y")];
        assert_eq!(state.add_streamed(both).unwrap(), 2);

        let counts = streamed_counts(&state.streamed());
        assert_eq!(counts.len(), 1);
        assert_eq!((counts[0].service.as_str(), counts[0].songs), ("Spotify", 2));
    }

    #[test]
    fn an_entry_can_be_removed_and_a_service_forgotten_without_losing_another_services_finds() {
        let dir = TempDir::new().unwrap();
        let state = state(&dir);
        state
            .add_streamed(vec![
                streamed("Both", "A", "Spotify", "s"),
                streamed("Both", "A", "ListenBrainz", "l"),
                streamed("Spotify Only", "A", "Spotify", "s"),
                streamed("Removed", "A", "Spotify", "s"),
            ])
            .unwrap();

        let id = state.streamed()[2].id();
        state.remove_streamed(&id).unwrap();
        assert_eq!(state.forget_streamed("Spotify").unwrap(), 1);

        let rows = listed(&state);
        assert_eq!(rows, [("Both".to_owned(), "A".to_owned(), "l".to_owned())]);
        let again = CollectionState::load(dir.path().join("Snapshot.json"), dir.path().join("Streamed.json"));
        assert_eq!(listed(&again), rows);
    }

    #[test]
    fn an_album_is_filed_under_the_first_of_its_artists() {
        assert_eq!(primary_artist("Main, Guest, Another"), "Main");
        assert_eq!(primary_artist("Solo"), "Solo");
    }

    fn music_item(id: &str, title: &str, ownership: Ownership) -> MusicLibraryItem {
        MusicLibraryItem {
            persistent_id: id.into(),
            title: title.into(),
            artist: "Band".into(),
            album_artist: String::new(),
            album: "Album".into(),
            genre: String::new(),
            duration_ms: None,
            track_number: None,
            disc_number: None,
            year: None,
            kind: "AAC audio file".into(),
            media: MediaType::Music,
            ownership,
            location: None,
            file_present: false,
            play_count: 0,
            skip_count: 0,
            rating: 0,
            date_added: None,
            last_played: None,
            last_skipped: None,
            enabled: true,
        }
    }

    #[test]
    fn a_streamed_song_the_music_library_lists_is_that_row_and_never_a_second_one_to_buy() {
        let dir = TempDir::new().unwrap();
        let library = LibraryState::open(dir.path().join("Library.sqlite"), dir.path().join("Artwork")).unwrap();
        let snapshot = Snapshot {
            source: "Music".into(),
            loaded_at: 1,
            items: vec![music_item("A", "Bought", Ownership::PurchasedInCloud), music_item("B", "Subscribed", Ownership::Subscription)],
        };
        let songs = vec![
            streamed("Bought (Remastered)", "Album", "Spotify", "Spotify playlist “Road Trip”"),
            streamed("Subscribed", "", "ListenBrainz", "ListenBrainz listening history"),
            streamed("Neither", "Album", "Spotify", "Spotify playlist “Road Trip”"),
        ];

        let report = build_report(Some(&snapshot), &songs, &library).unwrap();

        let tracks: Vec<(&str, OwnStatus, Option<&str>)> = report
            .albums
            .iter()
            .flat_map(|album| &album.tracks)
            .map(|track| (track.title.as_str(), track.status, track.from.as_deref()))
            .collect();
        assert_eq!(
            tracks,
            [
                ("Bought", OwnStatus::Redownload, Some("Spotify playlist “Road Trip”")),
                ("Subscribed", OwnStatus::Buy, Some("ListenBrainz listening history")),
                ("Neither", OwnStatus::Buy, Some("Spotify playlist “Road Trip”")),
            ]
        );
        assert_eq!((report.totals.songs, report.totals.redownload, report.totals.buy), (3, 1, 2));
    }

    #[test]
    fn a_common_title_is_the_music_librarys_row_only_on_the_same_album() {
        let dir = TempDir::new().unwrap();
        let library = LibraryState::open(dir.path().join("Library.sqlite"), dir.path().join("Artwork")).unwrap();
        let snapshot =
            Snapshot { source: "Music".into(), loaded_at: 1, items: vec![music_item("A", "Intro", Ownership::PurchasedInCloud)] };
        let songs = vec![streamed("Intro", "Album", "Spotify", "x"), streamed("Intro", "Another Album", "Spotify", "y")];

        let report = build_report(Some(&snapshot), &songs, &library).unwrap();

        let tracks: Vec<(OwnStatus, Option<&str>)> =
            report.albums.iter().flat_map(|album| &album.tracks).map(|track| (track.status, track.from.as_deref())).collect();
        assert_eq!(tracks, [(OwnStatus::Redownload, Some("x")), (OwnStatus::Buy, Some("y"))]);
    }

    #[test]
    fn a_streamed_song_found_in_the_library_is_in_the_library() {
        let dir = TempDir::new().unwrap();
        let library = LibraryState::open(dir.path().join("Library.sqlite"), dir.path().join("Artwork")).unwrap();
        library
            .queries()
            .import_tracks(&[unsub_library::Track {
                title: "Owned".into(),
                artist: "Band".into(),
                album_artist: "Band".into(),
                album: "Album".into(),
                ..crate::test_support::track(1, "MP3", unsub_library::MediaKind::Music)
            }])
            .unwrap();
        let songs = vec![streamed("Owned", "Album", "Spotify", "x"), streamed("Wanted", "Album", "Spotify", "x")];

        let report = build_report(None, &songs, &library).unwrap();

        assert_eq!((report.totals.songs, report.totals.in_library, report.totals.buy), (2, 1, 1));
        assert_eq!(report.source, "");
        assert_eq!(report.streamed.len(), 1);
    }
}
