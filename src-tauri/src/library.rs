use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use antistreaming_library::{
    AlbumSummary, ArtistSummary, ComposerSummary, EditReport, GenreSummary, Library, LibraryStats, MediaSettings, Mix, OrganizeReport,
    Playlist, PlaylistKind, SmartRules, Track, TrackEdits, TrackQuery,
};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{CommandError, CommandResult};

pub const SCAN_PROGRESS_EVENT: &str = "library://scan-progress";
pub const CHANGED_EVENT: &str = "library://changed";
/// Organize Library's progress; `null` once it's done.
pub const ORGANIZE_EVENT: &str = "library://organize";

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

    /// Runs a change that touches files (tag writes, moves, the Trash) on its own connection,
    /// taking turns with imports, so the song list stays responsive meanwhile.
    fn with_writer<T>(&self, change: impl FnOnce(&mut Library) -> antistreaming_library::Result<T>) -> CommandResult<T> {
        let _turn = self.scan_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut library = Library::open(&self.db_path)?;
        Ok(change(&mut library)?)
    }

    /// Where the library keeps its files, defaulting to the folder rips, podcasts and purchases
    /// already go to.
    pub fn media_folder(&self, default: &Path) -> antistreaming_library::Result<PathBuf> {
        let mut library = self.queries();
        match library.media_settings()?.folder {
            Some(folder) => Ok(folder),
            None => {
                library.set_media_folder(default)?;
                Ok(default.to_path_buf())
            }
        }
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

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ArtworkChange {
    File { path: PathBuf },
    Remove,
}

/// Get Info's OK: writes the new tags into the files. Artwork comes as a file path so the
/// image never crosses the bridge as JSON.
#[tauri::command]
pub async fn library_edit_tracks(
    app: AppHandle,
    track_ids: Vec<i64>,
    mut edits: TrackEdits,
    artwork: Option<ArtworkChange>,
) -> CommandResult<EditReport> {
    edits.artwork = match artwork {
        Some(ArtworkChange::File { path }) => Some(Some(std::fs::read(&path).map_err(|e| CommandError::from(e.to_string()))?)),
        Some(ArtworkChange::Remove) => Some(None),
        None => None,
    };
    let report = app.state::<LibraryState>().with_writer(|library| library.edit_tracks(&track_ids, &edits))?;
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(report)
}

#[tauri::command]
pub fn library_media_settings(state: State<'_, LibraryState>) -> CommandResult<MediaSettings> {
    Ok(state.queries().media_settings()?)
}

#[tauri::command]
pub async fn library_set_media_settings(app: AppHandle, folder: PathBuf, organize: bool, copy_on_add: bool) -> CommandResult<()> {
    app.state::<LibraryState>().with_writer(|library| {
        library.set_media_folder(&folder)?;
        library.set_organize(organize)?;
        library.set_copy_on_add(copy_on_add)
    })
}

/// iTunes' Organize Library: moves every file in the media folder to where its tags say.
#[tauri::command]
pub fn library_organize(app: AppHandle) {
    std::thread::spawn(move || {
        let result: CommandResult<OrganizeReport> = app.state::<LibraryState>().with_writer(|library| {
            library.organize_now(|progress| {
                let _ = app.emit(ORGANIZE_EVENT, Some(progress));
            })
        });
        let _ = app.emit(ORGANIZE_EVENT, None::<()>);
        let message = match result {
            Ok(report) if report.failed.is_empty() => format!("Organized {} files", report.moved),
            Ok(report) => format!("Organized {} files; {} couldn't be moved", report.moved, report.failed.len()),
            Err(error) => format!("Couldn't organize the library: {error}"),
        };
        let _ = app.emit(crate::playback::PLAYER_EVENT, antistreaming_playback::PlayerEvent::Notice { message });
        let _ = app.emit(CHANGED_EVENT, ());
    });
}

/// Groups of duplicates, as iTunes' Display Duplicates (or Display Exact Duplicates) finds them.
#[tauri::command]
pub fn library_duplicates(state: State<'_, LibraryState>, exact: bool) -> CommandResult<Vec<Vec<i64>>> {
    Ok(state.queries().duplicates(exact)?)
}

/// Flags songs whose files have gone; run at launch, off the main thread.
pub fn check_missing(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || match app.state::<LibraryState>().with_writer(|library| library.check_missing()) {
        Ok(missing) if !missing.is_empty() => {
            let _ = app.emit(CHANGED_EVENT, ());
        }
        Ok(_) => {}
        Err(error) => eprintln!("[library] couldn't check for missing files: {error}"),
    });
}

/// Points a missing song at its file's new home, then looks in the same folder for the
/// others, as iTunes offered to. Returns how many more were found.
#[tauri::command]
pub async fn library_relink(app: AppHandle, track_id: i64, path: PathBuf, search_folder: bool) -> CommandResult<usize> {
    let found = app.state::<LibraryState>().with_writer(|library| {
        library.relink(track_id, &path)?;
        match path.parent().filter(|_| search_folder) {
            Some(folder) => Ok(library.relink_folder(folder)?.len()),
            None => Ok(0),
        }
    })?;
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(found)
}

#[tauri::command]
pub async fn library_delete(app: AppHandle, track_ids: Vec<i64>, move_files_to_trash: bool) -> CommandResult<()> {
    let result = app.state::<LibraryState>().with_writer(|library| library.delete_tracks(&track_ids, move_files_to_trash));
    // Some songs may have gone even if others couldn't be trashed.
    let _ = app.emit(CHANGED_EVENT, ());
    result
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
