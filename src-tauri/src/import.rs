use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use unsub_applemusic::{Located, MediaType, MusicLibrary, MusicLibraryItem, Ownership, PlaylistKind as MusicPlaylistKind};
use unsub_library::{ExternalStats, Library, MatchQuality, MediaKind, PlaylistKind, TrackOverrides, TrackQuery};

use crate::collection::external;
use crate::error::{CommandError, CommandResult};
use crate::library::{self, LibraryState};

/// Progress of the import for the LCD; `null` once it's done.
const PROGRESS_EVENT: &str = "import://progress";
/// Files handed to the library at a time: each is its own short transaction, and the LCD moves between them.
const ADD_CHUNK: usize = 500;

/// The library the sheet is showing, kept so Import doesn't have to read it a second time.
#[derive(Default)]
pub struct ImportState {
    music: Mutex<Option<MusicLibrary>>,
    busy: Arc<AtomicBool>,
}

/// Held while Music is read or imported; dropping it lets the next one start.
struct Busy(Arc<AtomicBool>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl ImportState {
    fn lock(&self) -> MutexGuard<'_, Option<MusicLibrary>> {
        self.music.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Two imports at once would duplicate each other's playlists and fight over the progress display.
    fn begin(&self) -> CommandResult<Busy> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(CommandError::from("Another import is still running.".to_owned()));
        }
        Ok(Busy(Arc::clone(&self.busy)))
    }
}

/// Which of our playlists came from which Music playlist, so importing again updates them in place.
#[derive(Default, Serialize, Deserialize)]
#[serde(transparent)]
struct PlaylistMapping {
    ids: HashMap<String, i64>,
    /// Where each playlist is recorded as soon as it's made, so a run that stops partway doesn't make
    /// them all again next time.
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl PlaylistMapping {
    fn load(path: &Path) -> Self {
        let mut mapping: Self = std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        mapping.path = Some(path.to_owned());
        mapping
    }

    fn insert(&mut self, persistent_id: String, playlist: i64) -> std::io::Result<()> {
        self.ids.insert(persistent_id, playlist);
        match &self.path {
            Some(path) => std::fs::write(path, serde_json::to_vec(self)?),
            None => Ok(()),
        }
    }
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    songs: usize,
    with_files: usize,
    /// Songs that should have a file but don't: no location, or one that has gone.
    missing: usize,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum ImportStage {
    Reading,
    Locating,
    Adding,
    Merging,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportProgress {
    stage: ImportStage,
    done: usize,
    /// Unknown while listing the folder or reading the library.
    total: Option<usize>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    songs: usize,
    matched: usize,
    /// Songs that got a file of their own (new to the library).
    added: usize,
    /// Of those, the ones whose file was found in the chosen folder.
    located: usize,
    /// Songs that should have a file but none was found for.
    not_found: usize,
    /// Added songs whose file is only a placeholder in iCloud.
    in_icloud: usize,
    /// Files `add_files` couldn't add.
    failed: usize,
    playlists_created: usize,
    playlists_updated: usize,
    /// Playlist entries whose song isn't in Unsub (yet); they're left out, not faked.
    entries_skipped: usize,
}

/// Music stores half stars (10, 30, …); Unsub's ratings are whole stars like iTunes 10.
fn whole_stars(rating: u8) -> u8 {
    ((u16::from(rating) + 10) / 20 * 20).min(100) as u8
}

/// What kind of song this is here, or `None` for the items that never have a file to bring in:
/// streams, and the videos and other media Unsub has no kind for.
fn file_kind(item: &MusicLibraryItem) -> Option<MediaKind> {
    if item.ownership == Ownership::Subscription {
        return None;
    }
    match item.media {
        MediaType::Music => Some(MediaKind::Music),
        MediaType::Podcast => Some(MediaKind::Podcast),
        MediaType::Audiobook => Some(MediaKind::Audiobook),
        MediaType::Video | MediaType::Other => None,
    }
}

/// Music's own details for a song, pinned over whatever its file says (a file in iCloud has no
/// tags to read). A field Music has nothing for stays `None` rather than blanking the file's.
fn overrides(item: &MusicLibraryItem, kind: MediaKind) -> TrackOverrides {
    let text = |value: &str| (!value.trim().is_empty()).then(|| value.to_owned());
    TrackOverrides {
        kind: Some(kind),
        title: text(&item.title),
        artist: text(&item.artist),
        album: text(&item.album),
        album_artist: text(&item.album_artist),
        genre: text(&item.genre),
        date_added: item.date_added,
        year: item.year,
        track_number: item.track_number,
        disc_number: item.disc_number,
        duration_ms: item.duration_ms,
        ..TrackOverrides::default()
    }
}

fn stats_of(item: &MusicLibraryItem) -> ExternalStats {
    ExternalStats {
        play_count: item.play_count,
        skip_count: item.skip_count,
        last_played: item.last_played,
        last_skipped: item.last_skipped,
        rating: whole_stars(item.rating),
        date_added: item.date_added,
        enabled: item.enabled,
    }
}

/// Duplicates in Music can end up on one file; combining their stats the same way in any order keeps
/// the result from depending on which came last.
fn merged(a: ExternalStats, b: ExternalStats) -> ExternalStats {
    ExternalStats {
        play_count: a.play_count.saturating_add(b.play_count),
        skip_count: a.skip_count.saturating_add(b.skip_count),
        last_played: a.last_played.max(b.last_played),
        last_skipped: a.last_skipped.max(b.last_skipped),
        rating: a.rating.max(b.rating),
        date_added: [a.date_added, b.date_added].into_iter().flatten().min(),
        enabled: a.enabled || b.enabled,
    }
}

fn preview(music: &MusicLibrary) -> ImportPreview {
    let songs = music.items.iter().filter(|item| file_kind(item).is_some());
    let with_files = songs.clone().filter(|item| item.file_present).count();
    let songs = songs.count();
    ImportPreview { songs, with_files, missing: songs - with_files }
}

/// The songs that have a file to add, in the order `add_files_in_place` is given them.
struct Plan<'a> {
    songs: Vec<&'a MusicLibraryItem>,
    files: Vec<(PathBuf, TrackOverrides)>,
    /// Songs that should have a file but neither Music nor the folder has one.
    without_file: Vec<&'a MusicLibraryItem>,
}

impl<'a> Plan<'a> {
    fn new(music: &'a MusicLibrary, located: &Located) -> Self {
        let mut plan = Plan { songs: Vec::new(), files: Vec::new(), without_file: Vec::new() };
        for item in &music.items {
            let Some(kind) = file_kind(item) else { continue };
            // Where Music says the file is wins; the folder only fills in for what Music lost.
            let path = match &item.location {
                Some(location) if item.file_present => Some(location.clone()),
                _ => located.matched.get(&item.persistent_id).map(|found| found.path.clone()),
            };
            match path {
                Some(path) => {
                    plan.songs.push(item);
                    plan.files.push((path, overrides(item, kind)));
                }
                None => plan.without_file.push(item),
            }
        }
        plan
    }

    /// Persistent id → our track id for the songs `add_files_in_place` placed. A file that failed leaves its
    /// song out, so it's matched against the library like any song that has no file here.
    fn track_ids(&self, ids: &[Option<i64>]) -> HashMap<&'a str, i64> {
        self.songs.iter().zip(ids).filter_map(|(song, id)| Some((song.persistent_id.as_str(), (*id)?))).collect()
    }
}

fn import(
    library: &mut Library,
    music: &MusicLibrary,
    located: &Located,
    mapping: &mut PlaylistMapping,
    progress: &mut dyn FnMut(ImportProgress),
) -> unsub_library::Result<ImportSummary> {
    let everything = TrackQuery { kind: None, ..TrackQuery::default() };
    let plan = Plan::new(music, located);

    let before = library.stats(&everything)?.count;
    let total = Some(plan.files.len());
    let mut ids = Vec::with_capacity(plan.files.len());
    let mut failed = 0;
    for chunk in plan.files.chunks(ADD_CHUNK) {
        progress(ImportProgress { stage: ImportStage::Adding, done: ids.len(), total });
        let added = library.add_files_in_place(chunk)?;
        ids.extend(added.ids);
        failed += added.failed.len();
    }
    let mut ours = plan.track_ids(&ids);
    let placed: Vec<i64> = ours.values().copied().collect();
    let in_icloud = library.tracks(&TrackQuery { ids: Some(placed), ..everything.clone() })?.iter().filter(|track| track.in_cloud).count();

    progress(ImportProgress { stage: ImportStage::Merging, done: 0, total: Some(music.playlists.len()) });
    // A song with no file here may still be in the library from somewhere else. Only a sure match
    // counts, and never a track another song just got: that is a different version of it (a
    // greatest-hits copy) that would take the album copy's stats and playlist places.
    let claimed: HashSet<i64> = ours.values().copied().collect();
    let rest: Vec<_> = music.items.iter().filter(|item| !ours.contains_key(item.persistent_id.as_str())).collect();
    let externals: Vec<_> = rest.iter().map(|item| external(item)).collect();
    for (item, found) in rest.iter().zip(library.match_external(&externals)?) {
        if let Some(found) = found.filter(|found| found.quality != MatchQuality::Close && !claimed.contains(&found.track_id)) {
            ours.insert(item.persistent_id.as_str(), found.track_id);
        }
    }
    let mut stats: HashMap<i64, ExternalStats> = HashMap::new();
    for item in &music.items {
        if let Some(&id) = ours.get(item.persistent_id.as_str()) {
            let theirs = stats_of(item);
            stats.entry(id).and_modify(|earlier| *earlier = merged(earlier.clone(), theirs.clone())).or_insert(theirs);
        }
    }
    library.apply_external_stats(&stats.into_iter().collect::<Vec<_>>())?;

    let mut summary = ImportSummary {
        songs: music.items.len(),
        matched: ours.len(),
        added: (library.stats(&everything)?.count - before) as usize,
        located: located.matched.len(),
        not_found: plan.without_file.iter().filter(|item| !ours.contains_key(item.persistent_id.as_str())).count(),
        in_icloud,
        failed,
        ..ImportSummary::default()
    };
    let existing: Vec<i64> = library.playlists()?.into_iter().map(|p| p.id).collect();
    // Parents come before children in the export, so a folder's id is known before its contents.
    for (done, playlist) in music.playlists.iter().enumerate() {
        progress(ImportProgress { stage: ImportStage::Merging, done, total: Some(music.playlists.len()) });
        let folder = matches!(playlist.kind, MusicPlaylistKind::Folder);
        let kind = if folder { PlaylistKind::Folder } else { PlaylistKind::Regular };
        let parent = playlist.parent_persistent_id.as_ref().and_then(|p| mapping.ids.get(p)).copied();
        let id = match mapping.ids.get(&playlist.persistent_id).filter(|id| existing.contains(id)) {
            Some(&id) => {
                library.rename_playlist(id, &playlist.name)?;
                summary.playlists_updated += 1;
                id
            }
            None => {
                let created = library.create_playlist(&playlist.name, kind, parent)?;
                mapping.insert(playlist.persistent_id.clone(), created.id)?;
                summary.playlists_created += 1;
                created.id
            }
        };
        if folder {
            continue;
        }
        // Smart playlists arrive as their current members; Music's rule format isn't portable.
        let members: Vec<i64> = playlist.item_ids.iter().filter_map(|pid| ours.get(pid.as_str()).copied()).collect();
        summary.entries_skipped += playlist.item_ids.len() - members.len();
        // Every member counts here, not just music, so the whole old list is cleared.
        let query = TrackQuery { kind: None, playlist: Some(id), ..Default::default() };
        let current = library.tracks(&query)?.len() as u32;
        library.remove_from_playlist(id, &(0..current).collect::<Vec<_>>())?;
        library.add_to_playlist(id, &members, None)?;
    }
    Ok(summary)
}

fn songs(count: usize) -> String {
    format!("{count} {}", if count == 1 { "song" } else { "songs" })
}

fn report(app: &AppHandle, result: Result<ImportSummary, String>) {
    let (title, text, kind) = match result {
        Ok(s) => {
            let mut lines = Vec::new();
            if s.added > 0 {
                lines.push(format!("Added {} to your library.", songs(s.added)));
            }
            if s.located > 0 {
                lines.push(format!("{} found in the folder you chose.", songs(s.located)));
            }
            lines.push(format!("Matched {} of {} songs and merged their play counts, ratings and dates.", s.matched, s.songs));
            lines.push(format!("{} playlists created, {} updated.", s.playlists_created, s.playlists_updated));
            if s.not_found > 0 {
                lines.push(format!("No file was found for {}.", songs(s.not_found)));
            }
            if s.in_icloud > 0 {
                lines.push(format!("{} only in iCloud. They'll download when you play them.", songs(s.in_icloud)));
            }
            if s.failed > 0 {
                lines.push(format!("{} files couldn't be added.", s.failed));
            }
            if s.entries_skipped > 0 {
                lines.push(format!(
                    "{} playlist entries were left out because those songs aren't in Unsub yet; add them and import again.",
                    s.entries_skipped
                ));
            }
            ("Import Complete", lines.join("\n"), MessageDialogKind::Info)
        }
        Err(message) => ("Import Failed", message, MessageDialogKind::Error),
    };
    app.dialog().message(text).title(title).kind(kind).show(|_| {});
    let _ = app.emit(library::CHANGED_EVENT, ());
}

fn emit_progress(app: &AppHandle, progress: Option<ImportProgress>) {
    let _ = app.emit(PROGRESS_EVENT, progress);
}

fn read_music(xml: Option<PathBuf>) -> Result<MusicLibrary, String> {
    match xml {
        Some(path) => unsub_applemusic::read_xml_library(&path).map_err(|e| e.to_string()),
        #[cfg(target_os = "macos")]
        None => unsub_applemusic::read_system_music_library().map_err(|e| e.to_string()),
        #[cfg(not(target_os = "macos"))]
        None => Err("Reading the Music app directly needs macOS; choose an exported Library XML.".to_owned()),
    }
}

/// Looks for the songs whose files Music can't find. Only directories are listed, so nothing in
/// an iCloud folder is downloaded.
fn locate(music: &MusicLibrary, folder: &Path, progress: &mut dyn FnMut(ImportProgress)) -> unsub_applemusic::Result<Located> {
    let wanted: Vec<MusicLibraryItem> =
        music.items.iter().filter(|item| !item.file_present && file_kind(item).is_some()).cloned().collect();
    unsub_applemusic::locate_in_folder(&wanted, folder, &mut |found| {
        progress(ImportProgress { stage: ImportStage::Locating, done: found.done, total: found.total });
    })
}

fn run(app: &AppHandle, music: &MusicLibrary, folder: Option<&Path>) -> Result<ImportSummary, String> {
    let mut progress = |progress| emit_progress(app, Some(progress));
    let located = match folder {
        Some(folder) => locate(music, folder, &mut progress).map_err(|e| e.to_string())?,
        None => Located::default(),
    };
    let mapping_path = app.path().app_data_dir().map_err(|e| e.to_string())?.join("ImportedPlaylists.json");
    app.state::<LibraryState>()
        .with_writer(|library| {
            // Read under the writer's lock, so no other writer has an older copy to save over this one.
            let mut mapping = PlaylistMapping::load(&mapping_path);
            import(library, music, &located, &mut mapping, &mut progress)
        })
        .map_err(|e| e.to_string())
}

/// Reads the Music app (or an exported XML) and says what the import would find, keeping the
/// library for `import_music_run`.
#[tauri::command]
pub async fn import_music_inspect(app: AppHandle, state: State<'_, ImportState>, xml: Option<PathBuf>) -> CommandResult<ImportPreview> {
    let _busy = state.begin()?;
    emit_progress(&app, Some(ImportProgress { stage: ImportStage::Reading, done: 0, total: None }));
    // The first read of the Music app waits on the privacy prompt, so it stays off the IPC thread.
    let read = tauri::async_runtime::spawn_blocking(move || read_music(xml)).await;
    emit_progress(&app, None);
    let music = read.map_err(|e| CommandError::from(e.to_string()))?.map_err(CommandError::from)?;
    let preview = preview(&music);
    *state.lock() = Some(music);
    Ok(preview)
}

/// File › Import Music Library: brings in the songs of the library `import_music_inspect` read,
/// with their stats and playlists, looking in `folder` for the files Music has lost.
#[tauri::command]
pub fn import_music_run(app: AppHandle, state: State<'_, ImportState>, folder: Option<PathBuf>) -> CommandResult<()> {
    let busy = state.begin()?;
    let music = state.lock().take().ok_or_else(|| CommandError::from("The Music library wasn't read yet.".to_owned()))?;
    std::thread::spawn(move || {
        let _busy = busy;
        let result = run(&app, &music, folder.as_deref());
        emit_progress(&app, None);
        report(&app, result);
    });
    Ok(())
}

/// The sheet was dismissed, so the library it read isn't needed.
#[tauri::command]
pub fn import_music_cancel(state: State<'_, ImportState>) {
    state.lock().take();
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use unsub_applemusic::{Confidence, LocatedFile, MusicPlaylist};

    use super::*;
    use crate::test_support::track;

    fn song(id: &str, title: &str) -> MusicLibraryItem {
        MusicLibraryItem {
            persistent_id: id.to_owned(),
            title: title.to_owned(),
            artist: String::new(),
            album_artist: String::new(),
            album: String::new(),
            genre: String::new(),
            duration_ms: None,
            track_number: None,
            disc_number: None,
            year: None,
            kind: "AAC audio file".to_owned(),
            media: MediaType::Music,
            ownership: Ownership::MissingFile,
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

    fn library_of(items: Vec<MusicLibraryItem>) -> MusicLibrary {
        MusicLibrary { items, playlists: Vec::new() }
    }

    fn library_with_playlists(playlists: Vec<MusicPlaylist>) -> MusicLibrary {
        MusicLibrary { items: Vec::new(), playlists }
    }

    fn found(path: &str) -> LocatedFile {
        LocatedFile { path: PathBuf::from(path), confidence: Confidence::Exact, dataless: true }
    }

    #[test]
    fn half_stars_round_to_whole_stars() {
        assert_eq!(whole_stars(0), 0);
        assert_eq!(whole_stars(10), 20);
        assert_eq!(whole_stars(30), 40);
        assert_eq!(whole_stars(80), 80);
        assert_eq!(whole_stars(100), 100);
    }

    #[test]
    fn overrides_carry_everything_music_has() {
        let item = MusicLibraryItem {
            artist: "Artist".to_owned(),
            album_artist: "Band".to_owned(),
            album: "Album".to_owned(),
            genre: "Rock".to_owned(),
            year: Some(1999),
            track_number: Some(3),
            disc_number: Some(2),
            date_added: Some(1_000),
            ..song("A", "Title")
        };
        assert_eq!(
            overrides(&item, MediaKind::Audiobook),
            TrackOverrides {
                kind: Some(MediaKind::Audiobook),
                title: Some("Title".to_owned()),
                artist: Some("Artist".to_owned()),
                album: Some("Album".to_owned()),
                album_artist: Some("Band".to_owned()),
                genre: Some("Rock".to_owned()),
                date_added: Some(1_000),
                year: Some(1999),
                track_number: Some(3),
                disc_number: Some(2),
                ..TrackOverrides::default()
            }
        );
    }

    #[test]
    fn overrides_leave_out_what_music_does_not_have() {
        let item = MusicLibraryItem { artist: "  ".to_owned(), ..song("A", "") };
        assert_eq!(overrides(&item, MediaKind::Music), TrackOverrides { kind: Some(MediaKind::Music), ..TrackOverrides::default() });
    }

    #[test]
    fn the_kind_follows_music_s_media_type_and_streams_and_video_have_none() {
        let of = |media, ownership| file_kind(&MusicLibraryItem { media, ownership, ..song("A", "T") });
        assert_eq!(of(MediaType::Music, Ownership::LocalFile), Some(MediaKind::Music));
        assert_eq!(of(MediaType::Podcast, Ownership::UploadedInCloud), Some(MediaKind::Podcast));
        assert_eq!(of(MediaType::Audiobook, Ownership::MissingFile), Some(MediaKind::Audiobook));
        assert_eq!(of(MediaType::Video, Ownership::LocalFile), None);
        assert_eq!(of(MediaType::Other, Ownership::LocalFile), None);
        assert_eq!(of(MediaType::Music, Ownership::Subscription), None);
    }

    #[test]
    fn the_preview_counts_files_present_and_files_missing_and_skips_streams() {
        let present = MusicLibraryItem { location: Some(PathBuf::from("/music/a.m4a")), file_present: true, ..song("A", "Here") };
        let gone = MusicLibraryItem { location: Some(PathBuf::from("/old/b.m4a")), ..song("B", "Gone") };
        let never_had_one = song("C", "In iCloud");
        let stream = MusicLibraryItem { ownership: Ownership::Subscription, ..song("D", "Stream") };
        let video = MusicLibraryItem { media: MediaType::Video, ..song("E", "Clip") };
        let music = library_of(vec![present, gone, never_had_one, stream, video]);
        assert_eq!(preview(&music), ImportPreview { songs: 3, with_files: 1, missing: 2 });
    }

    #[test]
    fn a_song_uses_its_own_file_before_one_found_in_the_folder() {
        let own = MusicLibraryItem { location: Some(PathBuf::from("/music/own.m4a")), file_present: true, ..song("A", "Own") };
        let lost = song("B", "Lost");
        let nowhere = song("C", "Nowhere");
        let music = library_of(vec![own, lost, nowhere]);
        let located = Located {
            matched: HashMap::from([("A".to_owned(), found("/folder/other.m4a")), ("B".to_owned(), found("/folder/b.m4a"))]),
            unmatched: vec!["C".to_owned()],
        };
        let plan = Plan::new(&music, &located);
        let paths: Vec<&Path> = plan.files.iter().map(|(path, _)| path.as_path()).collect();
        assert_eq!(paths, [Path::new("/music/own.m4a"), Path::new("/folder/b.m4a")]);
        assert_eq!(plan.without_file.iter().map(|song| song.title.as_str()).collect::<Vec<_>>(), ["Nowhere"]);
    }

    #[test]
    fn a_file_that_failed_to_add_leaves_only_its_song_unmapped() {
        let music = library_of(vec![song("A", "One"), song("B", "Two"), song("C", "Three")]);
        let located = Located {
            matched: HashMap::from([
                ("A".to_owned(), found("/f/a.m4a")),
                ("B".to_owned(), found("/f/b.m4a")),
                ("C".to_owned(), found("/f/c.m4a")),
            ]),
            unmatched: Vec::new(),
        };
        let plan = Plan::new(&music, &located);
        assert_eq!(plan.track_ids(&[Some(11), None, Some(13)]), HashMap::from([("A", 11), ("C", 13)]));
    }

    /// A file named the way Music names it. It's empty and has no permissions, so anything that
    /// opens it fails, as it would for one that is only in iCloud.
    fn unreadable_file(root: &Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, []).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        path
    }

    fn playlist(id: &str, name: &str, kind: MusicPlaylistKind, parent: Option<&str>, items: &[&str]) -> MusicPlaylist {
        MusicPlaylist {
            persistent_id: id.to_owned(),
            name: name.to_owned(),
            parent_persistent_id: parent.map(str::to_owned),
            kind,
            item_ids: items.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    fn music_library(folder: &Path) -> MusicLibrary {
        let own_file = unreadable_file(folder, "Elsewhere/Singles/Existing.m4a");
        let items = vec![
            MusicLibraryItem {
                artist: "Artist".to_owned(),
                album: "Album".to_owned(),
                genre: "Rock".to_owned(),
                year: Some(2001),
                track_number: Some(1),
                play_count: 5,
                skip_count: 1,
                rating: 70,
                date_added: Some(1_000),
                last_played: Some(2_000),
                duration_ms: Some(200_000),
                ..song("P1", "Song One")
            },
            MusicLibraryItem {
                artist: "Show".to_owned(),
                album: "Show".to_owned(),
                track_number: Some(3),
                media: MediaType::Podcast,
                play_count: 2,
                ..song("P2", "Episode")
            },
            MusicLibraryItem {
                artist: "Elsewhere".to_owned(),
                album: "Singles".to_owned(),
                location: Some(own_file),
                file_present: true,
                ownership: Ownership::LocalFile,
                rating: 100,
                play_count: 9,
                enabled: false,
                ..song("P3", "Existing")
            },
            song("P4", "Lost Song"),
            // Already in the library, from a file somewhere else.
            MusicLibraryItem {
                artist: "Artist".to_owned(),
                album: "Album".to_owned(),
                duration_ms: Some(180_000),
                play_count: 4,
                skip_count: 1,
                last_played: Some(500),
                rating: 30,
                ..song("P5", "Song 9")
            },
            MusicLibraryItem { ownership: Ownership::Subscription, ..song("P6", "Stream") },
            MusicLibraryItem { media: MediaType::Video, ..song("P7", "Clip") },
            // The same title and artist as a song that got a file, on another album with no file: a
            // different version, which must not take the album copy's stats.
            MusicLibraryItem {
                artist: "Artist".to_owned(),
                album: "Greatest Hits".to_owned(),
                duration_ms: Some(200_000),
                play_count: 77,
                enabled: false,
                ..song("P8", "Song One")
            },
            // Only a loose match for the song in the library (no length, another album).
            MusicLibraryItem { artist: "Artist".to_owned(), album: "Other Album".to_owned(), play_count: 33, ..song("P9", "Song 9") },
            // A second Music entry for the song P5 is.
            MusicLibraryItem {
                artist: "Artist".to_owned(),
                album: "Album".to_owned(),
                duration_ms: Some(180_500),
                play_count: 6,
                skip_count: 2,
                last_played: Some(900),
                rating: 70,
                enabled: false,
                ..song("P10", "Song 9")
            },
        ];
        let playlists = vec![
            playlist("F1", "Folder", MusicPlaylistKind::Folder, None, &[]),
            playlist("L1", "Road", MusicPlaylistKind::Regular, Some("F1"), &["P1", "P4", "P3", "P5", "P1", "P8"]),
        ];
        MusicLibrary { items, playlists }
    }

    #[test]
    fn imports_songs_stats_and_playlists_from_files_that_cannot_be_opened() {
        let folder = TempDir::new().unwrap();
        unreadable_file(folder.path(), "Artist/Album/01 Song One.m4a");
        unreadable_file(folder.path(), "Show/Show/03 Episode.m4a");
        let music = music_library(folder.path());

        let mut library = Library::open_in_memory().unwrap();
        // Every file counts as a placeholder in iCloud, which the library must not open.
        library.set_dataless_check(|_| true);
        library.import_tracks(&[track(9, "AAC", MediaKind::Music)]).unwrap();
        let mut mapping = PlaylistMapping::default();

        let located = locate(&music, folder.path(), &mut |_| {}).unwrap();
        let summary = import(&mut library, &music, &located, &mut mapping, &mut |_| {}).unwrap();
        assert_eq!(
            (summary.added, summary.located, summary.not_found, summary.in_icloud, summary.matched, summary.failed),
            (3, 2, 3, 3, 5, 0)
        );
        assert_eq!((summary.playlists_created, summary.playlists_updated, summary.entries_skipped), (2, 0, 2));

        let everything = TrackQuery { kind: None, ..TrackQuery::default() };
        let tracks = library.tracks(&everything).unwrap();
        assert_eq!(tracks.len(), 4);
        let by_title = |title: &str| tracks.iter().find(|t| t.title == title).unwrap();

        let one = by_title("Song One");
        assert_eq!((one.kind, one.artist.as_str(), one.album.as_str(), one.genre.as_str()), (MediaKind::Music, "Artist", "Album", "Rock"));
        assert_eq!((one.year, one.track_number, one.date_added), (Some(2001), Some(1), 1_000));
        assert_eq!((one.play_count, one.skip_count, one.rating, one.last_played), (5, 1, 80, Some(2_000)));
        assert!(one.in_cloud && Path::new(&one.path).ends_with("Artist/Album/01 Song One.m4a"));
        // Music's length stands in for the one a placeholder can't read, and the greatest-hits copy
        // took none of the album copy's stats.
        assert_eq!((one.duration_ms, one.enabled), (200_000, true));
        assert_eq!(by_title("Episode").kind, MediaKind::Podcast);
        let existing = by_title("Existing");
        assert!(existing.in_cloud && !existing.enabled);
        assert_eq!((existing.play_count, existing.rating), (9, 100));
        // Two Music entries on the one song in the library merge, whichever came first; the loose match joins neither.
        let nine = by_title("Song 9");
        assert_eq!((nine.play_count, nine.skip_count, nine.last_played, nine.rating, nine.enabled), (10, 3, Some(900), 80, true));

        let road = mapping.ids["L1"];
        let playlists = library.playlists().unwrap();
        assert_eq!(playlists.iter().find(|p| p.id == road).unwrap().parent_id, Some(mapping.ids["F1"]));
        let mut members: Vec<i64> =
            library.tracks(&TrackQuery { playlist: Some(road), ..everything.clone() }).unwrap().iter().map(|t| t.id).collect();
        members.sort_unstable();
        let mut expected = vec![one.id, one.id, existing.id, 9];
        expected.sort_unstable();
        assert_eq!(members, expected);

        // Importing again updates in place and adds nothing.
        let again = import(&mut library, &music, &located, &mut mapping, &mut |_| {}).unwrap();
        assert_eq!((again.added, again.playlists_created, again.playlists_updated), (0, 0, 2));
        assert_eq!(library.tracks(&everything).unwrap().len(), 4);
    }

    #[test]
    fn songs_merged_onto_one_track_combine_the_same_way_in_either_order() {
        let a = ExternalStats {
            play_count: 4,
            skip_count: 1,
            last_played: Some(500),
            last_skipped: None,
            rating: 40,
            date_added: Some(30),
            enabled: false,
        };
        let b = ExternalStats {
            play_count: 6,
            skip_count: 0,
            last_played: Some(900),
            last_skipped: Some(7),
            rating: 20,
            date_added: None,
            enabled: true,
        };
        let expected = ExternalStats {
            play_count: 10,
            skip_count: 1,
            last_played: Some(900),
            last_skipped: Some(7),
            rating: 40,
            date_added: Some(30),
            enabled: true,
        };
        assert_eq!(merged(a.clone(), b.clone()), expected);
        assert_eq!(merged(b, a), expected);
    }

    #[test]
    fn a_second_import_is_refused_while_one_is_running() {
        let state = ImportState::default();
        let running = state.begin().unwrap();
        assert!(state.begin().is_err());
        drop(running);
        assert!(state.begin().is_ok());
    }

    #[test]
    fn ids_stay_with_their_songs_across_chunks_and_progress_follows_each_chunk() {
        let folder = TempDir::new().unwrap();
        let count = ADD_CHUNK * 2 + 100;
        let items = (0..count)
            .map(|n| MusicLibraryItem {
                location: Some(unreadable_file(folder.path(), &format!("Artist/Album/{n:04} Song {n}.m4a"))),
                file_present: true,
                play_count: n as u32,
                ..song(&format!("S{n}"), &format!("Song {n}"))
            })
            .collect();
        let music = library_of(items);
        let mut library = Library::open_in_memory().unwrap();
        library.set_dataless_check(|_| true);

        let mut adding = Vec::new();
        let mut watch = |progress: ImportProgress| {
            if matches!(progress.stage, ImportStage::Adding) {
                adding.push(progress.done);
            }
        };
        let summary = import(&mut library, &music, &Located::default(), &mut PlaylistMapping::default(), &mut watch).unwrap();

        assert_eq!((summary.added, summary.failed), (count, 0));
        assert_eq!(adding, [0, ADD_CHUNK, ADD_CHUNK * 2]);
        let tracks = library.tracks(&TrackQuery { kind: None, ..TrackQuery::default() }).unwrap();
        assert_eq!(tracks.len(), count);
        for track in &tracks {
            let n: u32 = track.title.strip_prefix("Song ").unwrap().parse().unwrap();
            assert_eq!(track.play_count, n, "{}", track.title);
        }
    }

    fn listing(dir: &Path, base: &Path, out: &mut Vec<(String, u64)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            let relative = entry.path().strip_prefix(base).unwrap().to_string_lossy().into_owned();
            out.push((relative, if metadata.is_dir() { 0 } else { metadata.len() }));
            if metadata.is_dir() {
                listing(&entry.path(), base, out);
            }
        }
    }

    #[test]
    fn local_files_are_never_copied_moved_or_renamed_whatever_the_media_folder_settings() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/library/tests/fixtures");
        // Where the library keeps its files, relative to a root with the chosen folder at `Music`:
        // around it, the same, and inside it.
        for media in ["", "Music", "Music/Media"] {
            let root = TempDir::new().unwrap();
            let chosen = root.path().join("Music");
            let halo = chosen.join("dump/halo.mp3");
            let something = chosen.join("loose/x.m4a");
            for (fixture, to) in [("halo.mp3", &halo), ("something.m4a", &something)] {
                std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                std::fs::copy(fixtures.join(fixture), to).unwrap();
            }
            let mut library = Library::open_in_memory().unwrap();
            library.set_media_folder(&root.path().join(media)).unwrap();
            library.set_organize(true).unwrap();
            library.set_copy_on_add(true).unwrap();
            let mut before = Vec::new();
            listing(root.path(), root.path(), &mut before);
            before.sort();

            let item = |id, title: &str, artist: &str, path: &PathBuf| MusicLibraryItem {
                artist: artist.to_owned(),
                album: "Album".to_owned(),
                location: Some(path.clone()),
                file_present: true,
                ownership: Ownership::LocalFile,
                ..song(id, title)
            };
            let music = library_of(vec![item("A", "Halo", "Someone", &halo), item("B", "Something", "Other", &something)]);
            for _ in 0..2 {
                import(&mut library, &music, &Located::default(), &mut PlaylistMapping::default(), &mut |_| {}).unwrap();
            }

            let mut after = Vec::new();
            listing(root.path(), root.path(), &mut after);
            after.sort();
            assert_eq!(after, before, "media folder {media:?}");
            let mut paths: Vec<String> =
                library.tracks(&TrackQuery { kind: None, ..TrackQuery::default() }).unwrap().into_iter().map(|t| t.path).collect();
            paths.sort();
            let mut expected: Vec<String> =
                [&halo, &something].iter().map(|path| std::fs::canonicalize(path).unwrap().to_string_lossy().into_owned()).collect();
            expected.sort();
            assert_eq!(paths, expected, "media folder {media:?}");
        }
    }

    #[test]
    fn playlists_made_before_a_failure_are_not_made_again_by_the_next_run() {
        let saved = TempDir::new().unwrap();
        let path = saved.path().join("ImportedPlaylists.json");
        let playlists = |inside: &str| {
            library_with_playlists(vec![
                playlist("F1", "Folder", MusicPlaylistKind::Folder, None, &[]),
                playlist("L1", "Road", MusicPlaylistKind::Regular, Some("F1"), &[]),
                playlist("L2", "Inside", MusicPlaylistKind::Regular, Some(inside), &[]),
            ])
        };
        let mut library = Library::open_in_memory().unwrap();

        // A playlist can't be put inside another playlist, so the run stops there.
        let stopped = import(&mut library, &playlists("L1"), &Located::default(), &mut PlaylistMapping::load(&path), &mut |_| {});
        assert!(stopped.is_err());

        let summary = import(&mut library, &playlists("F1"), &Located::default(), &mut PlaylistMapping::load(&path), &mut |_| {}).unwrap();
        assert_eq!((summary.playlists_created, summary.playlists_updated), (1, 2));
        let names: Vec<String> = library.playlists().unwrap().into_iter().map(|p| p.name).collect();
        for name in ["Folder", "Road", "Inside"] {
            assert_eq!(names.iter().filter(|n| *n == name).count(), 1, "{name}");
        }
    }
}
