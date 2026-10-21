use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use unsub_library::{
    AlbumSummary, ArtistSummary, Chapter, CleanUpChange, CleanUpRules, ComposerSummary, EditReport, GenreSummary, Library, LibraryStats,
    Lyrics, MediaSettings, Mix, OrganizeReport, Playlist, PlaylistKind, ScanSummary, SmartRules, Track, TrackEdits, TrackQuery,
};

use crate::error::{CommandError, CommandResult};
use crate::jobs::{self, Ctx, Job, Kind};

/// How long a statement of a long job waits for another connection's write.
const JOB_BUSY_TIMEOUT: Duration = Duration::from_secs(30);

pub const SCAN_PROGRESS_EVENT: &str = "library://scan-progress";
pub const CHANGED_EVENT: &str = "library://changed";
/// Organize Library's progress; `null` once it's done.
pub const ORGANIZE_EVENT: &str = "library://organize";
/// Clean Up Song Info's progress; `null` once it's done.
pub const CLEAN_UP_EVENT: &str = "library://clean-up";
/// How a clean-up ended, for the notice.
pub const CLEAN_UP_DONE_EVENT: &str = "library://clean-up-done";

/// SQLite in WAL mode lets each job keep its own connection, so a long import or a slow
/// artwork read never blocks the song list.
pub struct LibraryState {
    db_path: PathBuf,
    artwork_cache: PathBuf,
    queries: Mutex<Library>,
    artwork_reader: Mutex<Library>,
    /// Lyrics and chapters read the audio file, which can be slow (a big M4B's movie box), so they
    /// get a connection of their own rather than holding up `queries`.
    file_reader: Mutex<Library>,
    /// Serializes imports; a second drop waits for the first instead of racing it.
    scan_lock: Arc<Mutex<()>>,
}

impl LibraryState {
    pub fn open(db_path: PathBuf, artwork_cache: PathBuf) -> unsub_library::Result<Self> {
        let mut artwork_reader = Library::open(&db_path)?;
        artwork_reader.set_artwork_cache(artwork_cache.clone());
        Ok(Self {
            queries: Mutex::new(Library::open(&db_path)?),
            artwork_reader: Mutex::new(artwork_reader),
            file_reader: Mutex::new(Library::open(&db_path)?),
            db_path,
            artwork_cache,
            scan_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Other long-lived readers (the sharing server) open their own connection to the same file.
    pub fn db_path(&self) -> &std::path::Path {
        &self.db_path
    }

    /// A connection of its own for work that goes on for hours, so that it never holds up the song list's. It has the
    /// artwork cache, for the covers such work keeps there.
    pub fn job_connection(&self) -> unsub_library::Result<Library> {
        let mut library = Library::open(&self.db_path)?;
        library.set_artwork_cache(self.artwork_cache.clone());
        // Another writer holding a transaction for a while must not end work that has hours to go.
        library.set_busy_timeout(JOB_BUSY_TIMEOUT)?;
        Ok(library)
    }

    /// Runs a change that touches files (tag writes, moves, the Trash) on its own connection,
    /// taking turns with imports, so the song list stays responsive meanwhile.
    pub(crate) fn with_writer<T>(&self, change: impl FnOnce(&mut Library) -> unsub_library::Result<T>) -> CommandResult<T> {
        let _turn = self.scan_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut library = Library::open(&self.db_path)?;
        Ok(change(&mut library)?)
    }

    /// Where the library keeps its files, defaulting to the folder rips, podcasts and purchases
    /// already go to.
    pub fn media_folder(&self, default: &Path) -> unsub_library::Result<PathBuf> {
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

    fn file_reader(&self) -> MutexGuard<'_, Library> {
        self.file_reader.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The original art, or a cached JPEG thumbnail when the view only needs `size` pixels.
    pub fn artwork(&self, track_id: i64, size: Option<u32>) -> Option<unsub_library::Artwork> {
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

/// Get Info › Lyrics. Async so reading the file stays off the main thread.
#[tauri::command]
pub async fn library_lyrics(state: State<'_, LibraryState>, track_id: i64) -> CommandResult<Option<Lyrics>> {
    Ok(state.file_reader().lyrics(track_id)?)
}

/// The playing song's chapters for the LCD; an audiobook's file can be large, so async as well.
#[tauri::command]
pub async fn library_chapters(state: State<'_, LibraryState>, track_id: i64) -> CommandResult<Vec<Chapter>> {
    Ok(state.file_reader().chapters(track_id)?)
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

/// A cover is a few megabytes at most; this stops a path to something huge from being read whole.
const MAX_ARTWORK_BYTES: u64 = 20 << 20;

/// The bytes of a picture file, and nothing else: whatever file a path points to would otherwise be written into the songs.
fn read_picture(path: &Path) -> CommandResult<Vec<u8>> {
    let not_a_picture = || CommandError::from("That file isn't a picture".to_owned());
    let metadata = std::fs::metadata(path).map_err(|e| CommandError::from(e.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_ARTWORK_BYTES {
        return Err(not_a_picture());
    }
    // The file may have grown since it was measured.
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path).map_err(|e| CommandError::from(e.to_string()))?;
    file.take(MAX_ARTWORK_BYTES + 1).read_to_end(&mut bytes).map_err(|e| CommandError::from(e.to_string()))?;
    // Only a picture decodes; a file with a picture's first bytes but something else after them does not.
    if bytes.len() as u64 > MAX_ARTWORK_BYTES || unsub_library::shrink_to_jpeg(&bytes, 1).is_none() {
        return Err(not_a_picture());
    }
    Ok(bytes)
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
        Some(ArtworkChange::File { path }) => Some(Some(read_picture(&path)?)),
        Some(ArtworkChange::Remove) => Some(None),
        None => None,
    };
    let report = app.state::<LibraryState>().with_writer(|library| library.edit_tracks(&track_ids, &edits))?;
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(report)
}

/// How many of the changes Clean Up Song Info lists; the sheet only needs enough to scroll through.
const CLEAN_UP_LISTED: usize = 500;
/// Songs changed under one hold of the writer; between chunks it is let go, so Get Info, a rating or an import gets a turn.
const CLEAN_UP_CHUNK: usize = 200;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanUpPreview {
    /// Every song the rules would change, of which `changes` holds the first few.
    total: usize,
    /// How many of those are placeholders for files still in the cloud.
    in_cloud: usize,
    changes: Vec<CleanUpChange>,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct CleanUpProgress {
    done: u64,
    total: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CleanUpDone {
    report: EditReport,
    stopped: bool,
    /// Why it ended before every song was done, if it did.
    error: Option<String>,
}

/// Advanced › Clean Up Song Info, before the Apply: what would change, nothing written. Async because
/// a whole library is a lot of songs to read and run the rules over.
#[tauri::command]
pub async fn library_clean_up_preview(
    state: State<'_, LibraryState>,
    track_ids: Vec<i64>,
    rules: CleanUpRules,
) -> CommandResult<CleanUpPreview> {
    let mut changes = state.queries().clean_up_preview(&track_ids, &rules)?;
    let total = changes.len();
    let in_cloud = changes.iter().filter(|change| change.in_cloud).count();
    // What a change adds is easy to miss in a list this long, so those come first.
    changes.sort_by_key(|change| !change.fills());
    changes.truncate(CLEAN_UP_LISTED);
    Ok(CleanUpPreview { total, in_cloud, changes })
}

/// The Apply of Clean Up Song Info. Like Organize it runs in the background, and the LCD shows its progress
/// and can stop it; it takes the writer a chunk of songs at a time rather than for the whole run.
#[tauri::command]
pub fn library_clean_up(app: AppHandle, track_ids: Vec<i64>, rules: CleanUpRules) -> CommandResult<()> {
    let job = Job::new(Kind::CleanUp, CLEAN_UP_EVENT).changed(CHANGED_EVENT);
    jobs::start(&app, job, move |app, ctx| {
        let (report, error) = run_clean_up(&app.state::<LibraryState>(), ctx, &track_ids, &rules);
        ctx.emit(CLEAN_UP_DONE_EVENT, CleanUpDone { report, stopped: ctx.cancelled(), error });
    })
}

/// Changes the songs the rules touch, a chunk at a time with each chunk worked out from its rows as they are
/// by then. Returns what was done, and why it stopped early if an error did.
fn run_clean_up(state: &LibraryState, ctx: &Ctx, track_ids: &[i64], rules: &CleanUpRules) -> (EditReport, Option<String>) {
    let mut report = EditReport::default();
    let changed: Vec<i64> = match state.with_writer(|library| library.clean_up_preview(track_ids, rules)) {
        Ok(changes) => changes.into_iter().map(|change| change.id).collect(),
        Err(error) => return (report, Some(error.to_string())),
    };
    let total = changed.len() as u64;
    let mut finished = 0;
    let send = |done: u64| ctx.progress(done, Some(total), (), CleanUpProgress { done, total });
    send(0);
    for chunk in changed.chunks(CLEAN_UP_CHUNK) {
        if ctx.cancelled() {
            break;
        }
        match jobs::write(state, |library| library.clean_up(chunk, rules, ctx.cancel_flag(), |done, _| send(finished + done))) {
            Ok(part) => report.merge(part),
            Err(error) => return (report, Some(error.to_string())),
        }
        finished += chunk.len() as u64;
    }
    (report, None)
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
    let job = Job::new(Kind::Organize, ORGANIZE_EVENT).changed(CHANGED_EVENT);
    let _ = jobs::start(&app, job, |app, ctx| {
        let result: CommandResult<OrganizeReport> = app
            .state::<LibraryState>()
            .with_writer(|library| library.organize_now(|progress| ctx.progress(progress.done, Some(progress.total), (), progress)));
        ctx.notice(match result {
            Ok(report) if report.failed.is_empty() => format!("Organized {} files", report.moved),
            Ok(report) => format!("Organized {} files; {} couldn't be moved", report.moved, report.failed.len()),
            Err(error) => format!("Couldn't organize the library: {error}"),
        });
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

/// What a finished import tells the user. One that simply added songs says nothing, as in iTunes.
fn scan_notice(result: &Result<ScanSummary, impl std::fmt::Display>) -> Option<String> {
    match result {
        Ok(summary) if !summary.failed.is_empty() => {
            let count = summary.failed.len();
            Some(format!("{count} {} couldn't be added", if count == 1 { "file" } else { "files" }))
        }
        Ok(summary) if summary.scanned == 0 => Some("No music files were found".to_owned()),
        Ok(_) => None,
        Err(error) => Some(format!("Couldn't add to the library: {error}")),
    }
}

/// Imports on a background thread with its own connection, reporting progress to the LCD.
pub fn add_paths(app: &AppHandle, paths: Vec<PathBuf>) {
    let state = app.state::<LibraryState>();
    let (db_path, scan_lock) = (state.db_path.clone(), Arc::clone(&state.scan_lock));
    let job = Job::new(Kind::Scan, SCAN_PROGRESS_EVENT).open_ended().changed(CHANGED_EVENT);
    let _ = jobs::start(app, job, move |_, ctx| {
        let _turn = scan_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = Library::open(&db_path)
            .and_then(|mut library| library.add_paths(&paths, |progress| ctx.progress(progress.scanned, None, (), progress)));
        match &result {
            Ok(summary) if !summary.failed.is_empty() => {
                eprintln!("[library] {} files could not be read: {:?}", summary.failed.len(), summary.failed)
            }
            Ok(_) => {}
            Err(error) => eprintln!("[library] import failed: {error}"),
        }
        if let Some(message) = scan_notice(&result) {
            ctx.notice(message);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(scanned: u64, failed: usize) -> ScanSummary {
        ScanSummary { scanned, failed: vec![(String::new(), String::new()); failed], ..ScanSummary::default() }
    }

    #[test]
    fn an_import_that_found_songs_says_nothing() {
        assert_eq!(scan_notice(&Ok::<_, String>(summary(12, 0))), None);
    }

    #[test]
    fn an_import_that_found_no_audio_says_so() {
        assert_eq!(scan_notice(&Ok::<_, String>(summary(0, 0))).as_deref(), Some("No music files were found"));
    }

    #[test]
    fn files_that_could_not_be_read_are_counted() {
        assert_eq!(scan_notice(&Ok::<_, String>(summary(5, 1))).as_deref(), Some("1 file couldn't be added"));
        assert_eq!(scan_notice(&Ok::<_, String>(summary(5, 3))).as_deref(), Some("3 files couldn't be added"));
    }

    #[test]
    fn an_import_that_failed_shows_the_error() {
        assert_eq!(scan_notice(&Err::<ScanSummary, _>("disk full")).as_deref(), Some("Couldn't add to the library: disk full"));
    }

    /// A 1x1 PNG.
    const PNG: [u8; 70] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63,
        0x64, 0x60, 0xf8, 0x5f, 0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
        0xae, 0x42, 0x60, 0x82,
    ];

    fn picture_error(contents: &[u8]) -> Option<String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cover");
        std::fs::write(&path, contents).unwrap();
        read_picture(&path).err().map(|e| e.to_string())
    }

    #[test]
    fn a_picture_file_is_read_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cover.png");
        std::fs::write(&path, PNG).unwrap();
        assert_eq!(read_picture(&path).unwrap(), PNG);
    }

    #[test]
    fn a_file_that_is_not_a_picture_is_refused_before_it_can_be_put_in_a_song() {
        let refused = Some("That file isn't a picture".to_owned());
        assert_eq!(picture_error(b"-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----\n"), refused);
        // Starts like a picture, as a renamed text file can with "BM", but is not one.
        assert_eq!(picture_error(b"BM is not a picture, just text that starts that way"), refused);
        assert_eq!(picture_error(&[&PNG[..8], b"not the rest of a png"].concat()), refused);
        assert_eq!(picture_error(b""), refused);
    }

    #[test]
    fn a_huge_file_or_a_folder_is_refused_without_being_read() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.png");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_ARTWORK_BYTES + 1).unwrap();
        assert_eq!(read_picture(&big).unwrap_err().to_string(), "That file isn't a picture");
        assert_eq!(read_picture(dir.path()).unwrap_err().to_string(), "That file isn't a picture");
    }
}
