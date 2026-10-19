use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use antistreaming_applemusic::{MediaType, MusicLibraryItem, Ownership};
use antistreaming_library::{ExternalTrack, MatchQuality};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

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

pub struct CollectionState {
    path: PathBuf,
    snapshot: Mutex<Option<Snapshot>>,
}

impl CollectionState {
    pub fn load(path: PathBuf) -> Self {
        let snapshot = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Self { path, snapshot: Mutex::new(snapshot) }
    }

    fn snapshot(&self) -> MutexGuard<'_, Option<Snapshot>> {
        self.snapshot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum OwnStatus {
    InLibrary,
    Import,
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
    redownload: usize,
    buy: usize,
    unknown: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipReport {
    source: String,
    loaded_at: i64,
    totals: Totals,
    albums: Vec<AlbumGroup>,
}

fn status_of(item: &MusicLibraryItem, matched: bool) -> OwnStatus {
    if matched {
        return OwnStatus::InLibrary;
    }
    match item.ownership {
        Ownership::LocalFile => OwnStatus::Import,
        Ownership::PurchasedInCloud | Ownership::UploadedInCloud => OwnStatus::Redownload,
        Ownership::Subscription => OwnStatus::Buy,
        Ownership::Unknown => OwnStatus::Unknown,
    }
}

/// Only songs and audiobooks are things you'd buy to own here; video and podcasts aren't.
fn wanted(item: &MusicLibraryItem) -> bool {
    matches!(item.media, MediaType::Music | MediaType::Audiobook)
}

fn external(item: &MusicLibraryItem) -> ExternalTrack {
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
    }
}

fn build_report(snapshot: &Snapshot, library: &LibraryState) -> CommandResult<OwnershipReport> {
    let items: Vec<&MusicLibraryItem> = snapshot.items.iter().filter(|item| wanted(item)).collect();
    let externals: Vec<ExternalTrack> = items.iter().map(|item| external(item)).collect();
    let matches = library.queries().match_external(&externals)?;

    let mut totals = Totals { songs: items.len(), ..Totals::default() };
    // Case-insensitive album keys so "The Wall" and "the wall" land together.
    let mut albums: BTreeMap<(String, String), AlbumGroup> = BTreeMap::new();
    for (item, found) in items.into_iter().zip(matches) {
        let status = status_of(item, found.is_some());
        match status {
            OwnStatus::InLibrary => totals.in_library += 1,
            OwnStatus::Import => totals.import += 1,
            OwnStatus::Redownload => totals.redownload += 1,
            OwnStatus::Buy => totals.buy += 1,
            OwnStatus::Unknown => totals.unknown += 1,
        }
        let album_artist = if item.album_artist.is_empty() { item.artist.clone() } else { item.album_artist.clone() };
        let key = (album_artist.to_lowercase(), item.album.to_lowercase());
        let group = albums.entry(key).or_insert_with(|| AlbumGroup {
            album_artist,
            album: item.album.clone(),
            tracks: Vec::new(),
        });
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
        });
    }
    let albums = albums
        .into_values()
        .map(|mut group| {
            group.tracks.sort_by_key(|t| (t.disc_number.unwrap_or(1), t.track_number.unwrap_or(u32::MAX)));
            group
        })
        .collect();
    Ok(OwnershipReport { source: snapshot.source.clone(), loaded_at: snapshot.loaded_at, totals, albums })
}

#[tauri::command]
pub fn collection_report(state: State<'_, CollectionState>, library: State<'_, LibraryState>) -> CommandResult<Option<OwnershipReport>> {
    let snapshot = state.snapshot();
    snapshot.as_ref().map(|s| build_report(s, &library)).transpose()
}

fn finish_load(app: &AppHandle, source: &str, result: Result<Vec<MusicLibraryItem>, String>) {
    let outcome = result.and_then(|items| {
        app.state::<CollectionState>().replace(source, items).map_err(|e| format!("{e:?}"))
    });
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
        let result = antistreaming_applemusic::read_system_library().map_err(|e| e.to_string());
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
    app.dialog()
        .file()
        .set_title("Choose an Exported Library")
        .add_filter("Library XML", &["xml"])
        .pick_file(move |file| {
            let Some(path) = file.and_then(|f| f.into_path().ok()) else { return };
            std::thread::spawn(move || {
                let result = antistreaming_applemusic::read_xml(&path).map_err(|e| e.to_string());
                finish_load(&handle, "Library XML", result);
            });
        });
}

/// Adds every owned file the Music app has that AntiStreaming doesn't, in one import.
#[tauri::command]
pub fn collection_import_owned(app: AppHandle, state: State<'_, CollectionState>, library: State<'_, LibraryState>) -> CommandResult<usize> {
    let paths: Vec<PathBuf> = {
        let snapshot = state.snapshot();
        let Some(snapshot) = snapshot.as_ref() else { return Ok(0) };
        let report = build_report(snapshot, &library)?;
        let wanted: std::collections::HashSet<&str> = report
            .albums
            .iter()
            .flat_map(|album| &album.tracks)
            .filter(|track| track.status == OwnStatus::Import)
            .map(|track| track.persistent_id.as_str())
            .collect();
        snapshot
            .items
            .iter()
            .filter(|item| wanted.contains(item.persistent_id.as_str()))
            .filter_map(|item| item.location.clone())
            .collect()
    };
    let count = paths.len();
    if count > 0 {
        library::add_paths(&app, paths);
    }
    Ok(count)
}
