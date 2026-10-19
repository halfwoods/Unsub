use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use antistreaming_library::{Library, LibraryStats, Track, TrackQuery};
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
    pub fn open(db_path: PathBuf) -> antistreaming_library::Result<Self> {
        Ok(Self {
            queries: Mutex::new(Library::open(&db_path)?),
            artwork_reader: Mutex::new(Library::open(&db_path)?),
            db_path,
            scan_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn queries(&self) -> MutexGuard<'_, Library> {
        // A panic mid-query can't leave SQLite inconsistent (transactions roll back), so recover.
        self.queries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn artwork(&self, track_id: i64) -> Option<antistreaming_library::Artwork> {
        let library = self.artwork_reader.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        library.artwork(track_id).ok().flatten()
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
pub fn library_set_rating(state: State<'_, LibraryState>, track_id: i64, rating: u8) -> CommandResult<()> {
    Ok(state.queries().set_rating(track_id, rating)?)
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
