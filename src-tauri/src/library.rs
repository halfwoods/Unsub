use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use antistreaming_library::{
    AlbumSummary, ArtistSummary, ComposerSummary, GenreSummary, Library, LibraryStats, Mix, Playlist, PlaylistKind, SmartRules,
    Track, TrackQuery,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{CommandError, CommandResult};

pub const SCAN_PROGRESS_EVENT: &str = "library://scan-progress";
pub const CHANGED_EVENT: &str = "library://changed";

/// SQLite in WAL mode lets each job keep its own connection, so a long import or a slow
/// artwork read never blocks the song list.
pub struct LibraryState {
    db_path: PathBuf,
    queries: Mutex<Library>,
    artwork_reader: Mutex<Library>,
    /// Serializes imports; a second drop waits for the first instead of racing it.
    scan_lock: Arc<Mutex<()>>,
}

impl LibraryState {
    pub fn open(db_path: PathBuf, artwork_cache: PathBuf) -> antistreaming_library::Result<Self> {
        let mut artwork_reader = Library::open(&db_path)?;
        artwork_reader.set_artwork_cache(artwork_cache);
        Ok(Self {
            queries: Mutex::new(Library::open(&db_path)?),
            artwork_reader: Mutex::new(artwork_reader),
            db_path,
            scan_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Other long-lived readers (the sharing server) open their own connection to the same file.
    pub fn db_path(&self) -> &std::path::Path {
        &self.db_path
    }

    pub fn queries(&self) -> MutexGuard<'_, Library> {
        // A panic mid-query can't leave SQLite inconsistent (transactions roll back), so recover.
        self.queries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The original art, or a cached JPEG thumbnail when the view only needs `size` pixels.
    pub fn artwork(&self, track_id: i64, size: Option<u32>) -> Option<antistreaming_library::Artwork> {
        let library = self.artwork_reader.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match size {
            Some(size) => library.artwork_thumbnail(track_id, size),
            None => library.artwork(track_id),
        }
        .ok()
        .flatten()
    }

    pub fn track_path(&self, track_id: i64) -> CommandResult<PathBuf> {
        self.queries()
            .track(track_id)?
            .map(|track| PathBuf::from(track.path))
            .ok_or_else(|| CommandError::from(format!("track {track_id} is not in the library")))
    }
}

#[tauri::command]
pub fn library_tracks(state: State<'_, LibraryState>, query: TrackQuery) -> CommandResult<Vec<Track>> {
    Ok(state.queries().tracks(&query)?)
}

#[tauri::command]
pub fn library_track(state: State<'_, LibraryState>, track_id: i64) -> CommandResult<Option<Track>> {
    Ok(state.queries().track(track_id)?)
}

#[tauri::command]
pub fn library_stats(state: State<'_, LibraryState>, query: TrackQuery) -> CommandResult<LibraryStats> {
    Ok(state.queries().stats(&query)?)
}

#[tauri::command]
pub fn library_albums(state: State<'_, LibraryState>, query: TrackQuery) -> CommandResult<Vec<AlbumSummary>> {
    Ok(state.queries().albums(&query)?)
}

#[tauri::command]
pub fn library_artists(state: State<'_, LibraryState>, query: TrackQuery) -> CommandResult<Vec<ArtistSummary>> {
    Ok(state.queries().artists(&query)?)
}

#[tauri::command]
pub fn library_genres(state: State<'_, LibraryState>, query: TrackQuery) -> CommandResult<Vec<GenreSummary>> {
    Ok(state.queries().genres(&query)?)
}

#[tauri::command]
pub fn library_composers(state: State<'_, LibraryState>, query: TrackQuery) -> CommandResult<Vec<ComposerSummary>> {
    Ok(state.queries().composers(&query)?)
}

#[tauri::command]
pub fn library_set_enabled(state: State<'_, LibraryState>, track_ids: Vec<i64>, enabled: bool) -> CommandResult<()> {
    Ok(state.queries().set_enabled(&track_ids, enabled)?)
}

#[tauri::command]
pub fn library_album_tracks(state: State<'_, LibraryState>, key: String) -> CommandResult<Vec<Track>> {
    Ok(state.queries().album_tracks(&key)?)
}

#[tauri::command]
pub fn library_set_rating(state: State<'_, LibraryState>, track_id: i64, rating: u8) -> CommandResult<()> {
    Ok(state.queries().set_rating(track_id, rating)?)
}

/// The Mixes view: on-device "Genius Mixes" built from tags and listening history.
#[tauri::command]
pub async fn library_mixes(state: State<'_, LibraryState>) -> CommandResult<Vec<Mix>> {
    Ok(state.queries().mixes(12, 50)?)
}

/// "More like this" from the given songs, for the LCD's Mix button.
#[tauri::command]
pub fn library_mix_for(state: State<'_, LibraryState>, track_ids: Vec<i64>) -> CommandResult<Vec<i64>> {
    Ok(state.queries().mix_for(&track_ids, 50)?)
}

#[tauri::command]
pub fn library_playlists(state: State<'_, LibraryState>) -> CommandResult<Vec<Playlist>> {
    Ok(state.queries().playlists()?)
}

#[tauri::command]
pub fn library_create_playlist(
    state: State<'_, LibraryState>,
    name: String,
    kind: PlaylistKind,
    parent_id: Option<i64>,
    track_ids: Vec<i64>,
) -> CommandResult<Playlist> {
    let mut library = state.queries();
    let playlist = library.create_playlist(&name, kind, parent_id)?;
    if !track_ids.is_empty() {
        library.add_to_playlist(playlist.id, &track_ids, None)?;
    }
    Ok(playlist)
}

#[tauri::command]
pub fn library_rename_playlist(state: State<'_, LibraryState>, id: i64, name: String) -> CommandResult<()> {
    Ok(state.queries().rename_playlist(id, &name)?)
}

#[tauri::command]
pub fn library_delete_playlist(state: State<'_, LibraryState>, id: i64) -> CommandResult<()> {
    Ok(state.queries().delete_playlist(id)?)
}

#[tauri::command]
pub fn library_move_playlist(state: State<'_, LibraryState>, id: i64, parent_id: Option<i64>, index: u32) -> CommandResult<()> {
    Ok(state.queries().move_playlist(id, parent_id, index)?)
}

#[tauri::command]
pub fn library_add_to_playlist(state: State<'_, LibraryState>, id: i64, track_ids: Vec<i64>, at: Option<u32>) -> CommandResult<()> {
    Ok(state.queries().add_to_playlist(id, &track_ids, at)?)
}

#[tauri::command]
pub fn library_remove_from_playlist(state: State<'_, LibraryState>, id: i64, positions: Vec<u32>) -> CommandResult<()> {
    Ok(state.queries().remove_from_playlist(id, &positions)?)
}

#[tauri::command]
pub fn library_move_in_playlist(state: State<'_, LibraryState>, id: i64, positions: Vec<u32>, to: u32) -> CommandResult<()> {
    Ok(state.queries().move_in_playlist(id, &positions, to)?)
}

#[tauri::command]
pub fn library_set_smart_rules(state: State<'_, LibraryState>, id: i64, rules: SmartRules) -> CommandResult<()> {
    Ok(state.queries().set_smart_rules(id, &rules)?)
}

#[tauri::command]
pub fn library_add_paths(app: AppHandle, paths: Vec<PathBuf>) {
    add_paths(&app, paths);
}

/// Imports on a background thread with its own connection, reporting progress to the LCD.
pub fn add_paths(app: &AppHandle, paths: Vec<PathBuf>) {
    let state = app.state::<LibraryState>();
    let (db_path, scan_lock) = (state.db_path.clone(), Arc::clone(&state.scan_lock));
    let app = app.clone();
    std::thread::spawn(move || {
        let _turn = scan_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = Library::open(&db_path).and_then(|mut library| {
            library.add_paths(&paths, |progress| {
                let _ = app.emit(SCAN_PROGRESS_EVENT, progress);
            })
        });
        match result {
            Ok(summary) if !summary.failed.is_empty() => {
                eprintln!("[library] {} files could not be read: {:?}", summary.failed.len(), summary.failed)
            }
            Ok(_) => {}
            Err(error) => eprintln!("[library] import failed: {error}"),
        }
        let _ = app.emit(CHANGED_EVENT, ());
    });
}
