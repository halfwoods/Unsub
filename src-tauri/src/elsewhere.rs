use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use unsub_elsewhere::{Export, Play, Service, Song};
use unsub_library::{ExternalTrack, Library, MatchOutcome, PlaylistKind, TrackQuery};

use crate::collection::{CollectionState, Source, StreamedSong};
use crate::error::{CommandError, CommandResult};
use crate::import::{ImportProgress, ImportStage, ImportState, PlaylistMapping, emit_progress, name_key};
use crate::library::{self, LibraryState};

/// Plays go to the log this many to a transaction, so the LCD moves and a failure loses little.
/// Running it again is safe, since the log ignores plays it already has.
const PLAYS_CHUNK: usize = 5_000;
/// A song only the listening history names goes on the To Own list once it has been played this
/// many times. One listen is as likely a shuffle or a recommendation as a song worth owning, and a
/// long history would otherwise bury the songs from playlists, which someone chose, under
/// thousands of passing ones. Songs in a playlist are always listed.
const MIN_PLAYS_TO_WANT: usize = 2;
/// Exports are JSON and CSV; the rest of a Google Takeout (videos, photos) is never unpacked, and
/// a few gigabytes of what is wanted is far beyond any real history.
const EXPORT_FILES: &[&str] = &["json", "jsonl", "csv"];
const MAX_UNPACKED_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// What the sheet read, kept so Import doesn't have to read or match it a second time.
struct Prepared {
    service: Service,
    export: Export,
    matched: Matched,
}

#[derive(Default)]
pub struct ElsewhereState {
    read: Mutex<Option<Prepared>>,
}

impl ElsewhereState {
    fn lock(&self) -> MutexGuard<'_, Option<Prepared>> {
        self.read.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// What makes two songs in an export the same one to look up: title, artist, album, length, and
/// whether it is a play, since a play says how long it was heard and a playlist entry doesn't.
type SongKey<'a> = (&'a str, &'a str, Option<&'a str>, Option<u64>, bool);

/// Each distinct song is looked up once, however many playlists and plays name it.
#[derive(Default)]
struct Lookup<'a> {
    slots: HashMap<SongKey<'a>, usize>,
    externals: Vec<ExternalTrack>,
    /// How long each played song was heard for, play by play.
    heard: HashMap<usize, Vec<u64>>,
}

impl<'a> Lookup<'a> {
    fn slot(&mut self, key: SongKey<'a>) -> usize {
        let (title, artist, album, duration_ms, _) = key;
        *self.slots.entry(key).or_insert_with(|| {
            self.externals.push(ExternalTrack {
                external_id: self.externals.len().to_string(),
                title: title.to_owned(),
                artist: artist.to_owned(),
                album_artist: String::new(),
                album: album.unwrap_or_default().to_owned(),
                duration_ms,
                track_number: None,
                disc_number: None,
                path: None,
                heard_ms: None,
            });
            self.externals.len() - 1
        })
    }

    fn entry(&mut self, song: &'a Song) -> usize {
        self.slot((&song.title, &song.artist, song.album.as_deref(), song.duration_ms, false))
    }

    fn play(&mut self, play: &'a Play) -> usize {
        let slot = self.slot((&play.title, &play.artist, play.album.as_deref(), None, true));
        if let Some(ms) = play.ms_played {
            self.heard.entry(slot).or_default().push(ms);
        }
        slot
    }

    /// The songs to look up. A played song is at least as long as it is usually heard for: the
    /// middle of its plays, so a few left running don't make it longer than it is.
    fn finish(mut self) -> Vec<ExternalTrack> {
        for (slot, mut played) in self.heard {
            played.sort_unstable();
            self.externals[slot].heard_ms = Some(played[played.len() / 2]);
        }
        self.externals
    }
}

/// How the songs of an export fared against the library. Playlist entries and plays point at a
/// distinct song by number, so a song named by thousands of plays is matched and kept once.
struct Matched {
    outcomes: Vec<MatchOutcome>,
    /// For each playlist, the song at each of its entries.
    playlists: Vec<Vec<usize>>,
    /// The song of each play.
    plays: Vec<usize>,
}

impl Matched {
    /// Matches strictly: a song that fits several albums equally well is left alone rather than
    /// given to whichever is first, since a wrong playlist entry or play is worse than a missing one.
    fn new(library: &Library, export: &Export) -> unsub_library::Result<Self> {
        let mut lookup = Lookup::default();
        let playlists = export.playlists.iter().map(|playlist| playlist.songs.iter().map(|song| lookup.entry(song)).collect()).collect();
        let plays = export.plays.iter().map(|play| lookup.play(play)).collect();
        Ok(Matched { outcomes: library.match_external_strict(&lookup.finish())?, playlists, plays })
    }

    /// The library still has every track this found. It does unless something was deleted since
    /// the sheet looked, which would make a playlist entry fail.
    fn still_valid(&self, library: &Library) -> unsub_library::Result<bool> {
        let tracks: HashSet<i64> = self.outcomes.iter().filter_map(MatchOutcome::track_id).collect();
        let query = TrackQuery { kind: None, ids: Some(tracks.iter().copied().collect()), ..TrackQuery::default() };
        Ok(tracks.is_empty() || library.tracks(&query)?.len() == tracks.len())
    }

    fn track(&self, song: usize) -> Option<i64> {
        self.outcomes[song].track_id()
    }

    fn is_missing(&self, song: usize) -> bool {
        matches!(self.outcomes[song], MatchOutcome::Missing)
    }

    /// How many of these songs were found, and how many couldn't be told apart.
    fn tally(&self, songs: &[usize]) -> (usize, usize) {
        let found = songs.iter().filter(|song| self.track(**song).is_some()).count();
        (found, songs.iter().filter(|song| self.outcomes[**song].is_ambiguous()).count())
    }
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElsewherePreview {
    playlists: usize,
    /// Entries in playlists, counting a song once for each playlist it is in.
    playlist_songs: usize,
    playlist_matched: usize,
    playlist_ambiguous: usize,
    plays: usize,
    plays_matched: usize,
    plays_ambiguous: usize,
    /// Plays under Spotify's 30 seconds, which weren't counted.
    short_plays: usize,
    /// Playlist entries that were only a video id, which nothing in the export could name.
    unnamed: usize,
}

fn preview(export: &Export, matched: &Matched) -> ElsewherePreview {
    let (mut playlist_songs, mut playlist_matched, mut playlist_ambiguous) = (0, 0, 0);
    for songs in &matched.playlists {
        let (found, unsure) = matched.tally(songs);
        playlist_songs += songs.len();
        playlist_matched += found;
        playlist_ambiguous += unsure;
    }
    let (plays_matched, plays_ambiguous) = matched.tally(&matched.plays);
    ElsewherePreview {
        playlists: export.playlists.len(),
        playlist_songs,
        playlist_matched,
        playlist_ambiguous,
        plays: matched.plays.len(),
        plays_matched,
        plays_ambiguous,
        short_plays: export.short_plays,
        unnamed: export.unnamed,
    }
}

#[derive(Debug, Default)]
pub struct ElsewhereSummary {
    playlists_created: usize,
    playlists_updated: usize,
    playlist_songs: usize,
    playlist_matched: usize,
    playlist_ambiguous: usize,
    plays: usize,
    plays_matched: usize,
    plays_ambiguous: usize,
    /// Plays that weren't in the play log already.
    plays_added: usize,
    /// Songs newly put on the To Own list.
    to_own: usize,
    short_plays: usize,
    unnamed: usize,
}

/// A song the export named that the library doesn't have, labelled with where the export had it.
/// `CollectionState` keeps one of each.
fn wanted(service: Service, label: &str, title: &str, artist: &str, album: Option<&str>, duration_ms: Option<u64>) -> StreamedSong {
    StreamedSong {
        title: title.to_owned(),
        artist: artist.to_owned(),
        album: album.unwrap_or_default().to_owned(),
        duration_ms,
        sources: vec![Source { service: service.name().to_owned(), label: label.to_owned() }],
    }
}

/// The songs only the history names, that the library lacks, once each, and heard often enough to
/// want. Playlists list theirs separately.
fn heard_often(service: Service, export: &Export, matched: &Matched) -> Vec<StreamedSong> {
    let label = format!("{} listening history", service.name());
    let mut heard: HashMap<usize, (usize, usize)> = HashMap::new();
    let mut order = Vec::new();
    for (index, &song) in matched.plays.iter().enumerate().filter(|(_, song)| matched.is_missing(**song)) {
        let (_, plays) = heard.entry(song).or_insert_with(|| {
            order.push(song);
            (index, 0)
        });
        *plays += 1;
    }
    order
        .into_iter()
        .filter_map(|song| {
            let (first, plays) = heard[&song];
            let play = &export.plays[first];
            (plays >= MIN_PLAYS_TO_WANT).then(|| wanted(service, &label, &play.title, &play.artist, play.album.as_deref(), None))
        })
        .collect()
}

/// Puts the export's playlists in a folder named for the service, and (unless `add_plays` is off)
/// its plays in the play log, using only songs that are certainly the ones in the library. What
/// couldn't be matched is returned, to be listed in To Own.
///
/// Importing again is safe: playlists are found by name through `mapping` and refilled instead of
/// made again, and the play log ignores a play it already has.
fn import(
    library: &mut Library,
    service: Service,
    export: &Export,
    matched: &Matched,
    add_plays: bool,
    mapping: &mut PlaylistMapping,
    progress: &mut dyn FnMut(ImportProgress),
) -> unsub_library::Result<(ElsewhereSummary, Vec<StreamedSong>)> {
    let mut summary = ElsewhereSummary { short_plays: export.short_plays, unnamed: export.unnamed, ..ElsewhereSummary::default() };
    let mut missing = Vec::new();
    let tag = service.tag();

    if !export.playlists.is_empty() {
        let existing = library.playlists()?;
        let (folder, _) =
            mapping.place(library, &existing, format!("{tag}:"), &format!("From {}", service.name()), PlaylistKind::Folder, None)?;
        // Two playlists can share a name, and neither export gives them an id to tell them apart.
        // "road_trip.csv" and "Road Trip" are the same playlist.
        let mut seen: HashMap<String, usize> = HashMap::new();
        let total = Some(export.playlists.len());
        for (done, (playlist, songs)) in export.playlists.iter().zip(&matched.playlists).enumerate() {
            progress(ImportProgress { stage: ImportStage::Applying, done, total });
            let folded = name_key(&playlist.name);
            let nth = seen.entry(folded.clone()).and_modify(|nth| *nth += 1).or_insert(1);
            let key = if *nth == 1 { format!("{tag}:{folded}") } else { format!("{tag}:{folded}#{nth}") };
            let (id, created) = mapping.place(library, &existing, key, &playlist.name, PlaylistKind::Regular, Some(folder))?;
            if created {
                summary.playlists_created += 1;
            } else {
                summary.playlists_updated += 1;
            }
            let members: Vec<i64> = songs.iter().filter_map(|song| matched.track(*song)).collect();
            library.set_playlist_items(id, &members)?;

            let label = format!("{} playlist “{}”", service.name(), playlist.name);
            let mut listed = HashSet::new();
            for (song, &slot) in playlist.songs.iter().zip(songs) {
                if matched.is_missing(slot) && listed.insert(slot) {
                    missing.push(wanted(service, &label, &song.title, &song.artist, song.album.as_deref(), song.duration_ms));
                }
            }
            let (found, unsure) = matched.tally(songs);
            summary.playlist_songs += songs.len();
            summary.playlist_matched += found;
            summary.playlist_ambiguous += unsure;
        }
    }

    if add_plays {
        let heard: Vec<(i64, i64)> =
            export.plays.iter().zip(&matched.plays).filter_map(|(play, song)| Some((matched.track(*song)?, play.at))).collect();
        let total = Some(heard.len());
        for (chunk, plays) in heard.chunks(PLAYS_CHUNK).enumerate() {
            progress(ImportProgress { stage: ImportStage::Plays, done: chunk * PLAYS_CHUNK, total });
            summary.plays_added += library.record_plays_from(tag, plays)?;
        }
        summary.plays = matched.plays.len();
        summary.plays_matched = heard.len();
        summary.plays_ambiguous = matched.tally(&matched.plays).1;
        missing.extend(heard_often(service, export, matched));
    }
    Ok((summary, missing))
}

fn plural(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn report(app: &AppHandle, service: Service, result: Result<ElsewhereSummary, String>) {
    let (title, text, kind) = match result {
        Ok(s) => {
            let mut lines = Vec::new();
            if s.plays_added > 0 {
                lines.push(format!("Added {} to your play counts.", plural(s.plays_added, "play", "plays")));
            } else if s.plays_matched > 0 {
                lines.push("Those plays were already in your play counts.".to_owned());
            }
            if s.plays > 0 {
                lines.push(format!("Matched {} of {} plays to songs you own.", s.plays_matched, s.plays));
            }
            if s.playlists_created + s.playlists_updated > 0 {
                lines.push(format!(
                    "{} created, {} updated, in “From {}”.",
                    plural(s.playlists_created, "playlist", "playlists"),
                    s.playlists_updated,
                    service.name()
                ));
                lines.push(format!("Matched {} of {} playlist songs.", s.playlist_matched, s.playlist_songs));
            }
            if s.to_own > 0 {
                lines.push(format!(
                    "{} you don't have {} added to To Own.",
                    plural(s.to_own, "song", "songs"),
                    if s.to_own == 1 { "was" } else { "were" }
                ));
            }
            let unsure = s.playlist_ambiguous + s.plays_ambiguous;
            if unsure > 0 {
                lines.push(format!(
                    "{} fit more than one of your songs equally well, so they were left out.",
                    plural(unsure, "entry", "entries")
                ));
            }
            if s.short_plays > 0 {
                lines.push(format!("{} under 30 seconds weren't counted.", plural(s.short_plays, "play", "plays")));
            }
            if s.unnamed > 0 {
                lines.push(format!("{} in playlists only had a video ID, and couldn't be named.", plural(s.unnamed, "song", "songs")));
            }
            ("Import Complete", lines.join("\n"), MessageDialogKind::Info)
        }
        Err(message) => ("Import Failed", message, MessageDialogKind::Error),
    };
    app.dialog().message(text).title(title).kind(kind).show(|_| {});
    let _ = app.emit(library::CHANGED_EVENT, ());
}

/// A ZIP is unpacked into a folder that goes when the read is over; the export has the person's IP
/// addresses in it, which should not outlive the import. `unpacking` is told bytes done and total.
fn read_export(service: Service, path: &Path, unpacking: &mut dyn FnMut(u64, u64)) -> Result<Export, String> {
    if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("zip")) {
        let unpacked = tempfile::tempdir().map_err(|e| e.to_string())?;
        unsub_purchases::unpack(path, unpacked.path(), EXPORT_FILES, MAX_UNPACKED_BYTES, unpacking).map_err(|e| e.to_string())?;
        return unsub_elsewhere::read(service, unpacked.path()).map_err(|e| e.to_string());
    }
    unsub_elsewhere::read(service, path).map_err(|e| e.to_string())
}

fn run(app: &AppHandle, prepared: &Prepared, add_plays: bool) -> Result<ElsewhereSummary, String> {
    let mapping_path = app.path().app_data_dir().map_err(|e| e.to_string())?.join("ImportedPlaylists.json");
    let mut progress = |progress| emit_progress(app, Some(progress));
    let (mut summary, wanted) = app
        .state::<LibraryState>()
        .with_writer(|library| {
            // Read under the writer's lock, so no other writer has an older copy to save over this one.
            let mut mapping = PlaylistMapping::load(&mapping_path);
            // What the sheet found stands unless a track it found has since gone.
            let fresh;
            let matched = if prepared.matched.still_valid(library)? {
                &prepared.matched
            } else {
                progress(ImportProgress { stage: ImportStage::Matching, done: 0, total: None });
                fresh = Matched::new(library, &prepared.export)?;
                &fresh
            };
            import(library, prepared.service, &prepared.export, matched, add_plays, &mut mapping, &mut progress)
        })
        .map_err(|e| e.to_string())?;
    summary.to_own = app.state::<CollectionState>().add_streamed(wanted).map_err(|e| e.to_string())?;
    Ok(summary)
}

/// Reads an export (a file, a folder, or a ZIP of either) and says what importing it would find,
/// keeping what it read for `elsewhere_run`.
#[tauri::command]
pub async fn elsewhere_inspect(
    app: AppHandle,
    imports: State<'_, ImportState>,
    state: State<'_, ElsewhereState>,
    service: Service,
    path: PathBuf,
) -> CommandResult<ElsewherePreview> {
    let _busy = imports.begin()?;
    emit_progress(&app, Some(ImportProgress { stage: ImportStage::ReadingExport, done: 0, total: None }));
    let handle = app.clone();
    let read = tauri::async_runtime::spawn_blocking(move || {
        // A big ZIP takes a while, so the LCD follows it; once a percent is enough.
        let mut shown = u64::MAX;
        let mut unpacking = |done: u64, total: u64| {
            let percent = if total == 0 { 100 } else { done * 100 / total };
            if percent != shown {
                shown = percent;
                emit_progress(
                    &handle,
                    Some(ImportProgress { stage: ImportStage::ReadingExport, done: done as usize, total: Some(total as usize) }),
                );
            }
        };
        let export = read_export(service, &path, &mut unpacking)?;
        emit_progress(&handle, Some(ImportProgress { stage: ImportStage::Matching, done: 0, total: None }));
        let matched = Matched::new(&handle.state::<LibraryState>().queries(), &export).map_err(|e| e.to_string())?;
        Ok::<_, String>(Prepared { service, export, matched })
    })
    .await;
    emit_progress(&app, None);
    let prepared = read.map_err(|e| CommandError::from(e.to_string()))?.map_err(CommandError::from)?;
    let found = preview(&prepared.export, &prepared.matched);
    *state.lock() = Some(prepared);
    Ok(found)
}

/// File › Import from Spotify, YouTube Music or ListenBrainz: brings in what `elsewhere_inspect` read.
#[tauri::command]
pub fn elsewhere_run(
    app: AppHandle,
    imports: State<'_, ImportState>,
    state: State<'_, ElsewhereState>,
    add_plays: bool,
) -> CommandResult<()> {
    let busy = imports.begin()?;
    let prepared = state.lock().take().ok_or_else(|| CommandError::from("The export wasn't read yet.".to_owned()))?;
    std::thread::spawn(move || {
        let _busy = busy;
        let result = run(&app, &prepared, add_plays);
        emit_progress(&app, None);
        report(&app, prepared.service, result);
    });
    Ok(())
}

/// The sheet was dismissed, so what it read isn't needed.
#[tauri::command]
pub fn elsewhere_cancel(state: State<'_, ElsewhereState>) {
    state.lock().take();
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::TempDir;
    use unsub_elsewhere::{Play, Playlist, Song};
    use unsub_library::MediaKind;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::*;
    use crate::test_support::track;

    fn owned_song(id: i64, title: &str, artist: &str, album: &str) -> unsub_library::Track {
        unsub_library::Track {
            title: title.into(),
            artist: artist.into(),
            album_artist: artist.into(),
            album: album.into(),
            ..track(id, "MP3", MediaKind::Music)
        }
    }

    fn library() -> Library {
        let mut library = Library::open_in_memory().unwrap();
        library
            .import_tracks(&[
                owned_song(1, "Fixture Song", "The Fixtures", "Test Album"),
                owned_song(2, "Second Song", "Sample Band", "Second Album"),
                owned_song(3, "Twin", "Sample Band", "Original"),
                owned_song(4, "Twin", "Sample Band", "Greatest Hits"),
            ])
            .unwrap();
        library
    }

    fn song(title: &str, artist: &str, album: Option<&str>) -> Song {
        Song { title: title.into(), artist: artist.into(), album: album.map(Into::into), duration_ms: None, service_id: None }
    }

    fn play(at: i64, title: &str, artist: &str, album: Option<&str>) -> Play {
        Play { at, title: title.into(), artist: artist.into(), album: album.map(Into::into), ms_played: None }
    }

    fn export() -> Export {
        Export {
            playlists: vec![
                Playlist {
                    name: "Road Trip".into(),
                    songs: vec![
                        song("Fixture Song", "The Fixtures", Some("Test Album")),
                        song("Unowned Song", "Sample Band", Some("New Album")),
                        song("Twin", "Sample Band", None),
                        song("Fixture Song", "The Fixtures", None),
                        song("Unowned Song", "Sample Band", Some("New Album")),
                    ],
                },
                Playlist { name: "Road Trip".into(), songs: vec![song("Second Song", "Sample Band", Some("Second Album"))] },
            ],
            plays: vec![
                play(100, "Fixture Song", "The Fixtures", Some("Test Album")),
                play(200, "Fixture Song", "The Fixtures", Some("Test Album")),
                play(300, "Unowned Song", "Sample Band", None),
                play(350, "Heard Only Once", "Sample Band", Some("Elsewhere")),
                play(360, "Heard Often", "Sample Band", Some("Elsewhere")),
                play(370, "Heard Often", "Sample Band", Some("Elsewhere")),
                play(400, "Twin", "Sample Band", None),
                play(500, "Twin", "Sample Band", Some("Greatest Hits")),
            ],
            short_plays: 2,
            unnamed: 1,
        }
    }

    fn members(library: &Library, playlist: i64) -> Vec<i64> {
        let query = TrackQuery { kind: None, playlist: Some(playlist), ..TrackQuery::default() };
        library.tracks(&query).unwrap().into_iter().map(|track| track.id).collect()
    }

    fn named(library: &Library, name: &str) -> Vec<unsub_library::Playlist> {
        library.playlists().unwrap().into_iter().filter(|playlist| playlist.name == name).collect()
    }

    fn import_again(library: &mut Library, mapping: &mut PlaylistMapping, add_plays: bool) -> (ElsewhereSummary, Vec<StreamedSong>) {
        import_export(library, Service::Spotify, &export(), mapping, add_plays)
    }

    fn import_export(
        library: &mut Library,
        service: Service,
        export: &Export,
        mapping: &mut PlaylistMapping,
        add_plays: bool,
    ) -> (ElsewhereSummary, Vec<StreamedSong>) {
        let matched = Matched::new(library, export).unwrap();
        import(library, service, export, &matched, add_plays, mapping, &mut |_| {}).unwrap()
    }

    fn labelled(wanted: &[StreamedSong]) -> Vec<(&str, String)> {
        wanted.iter().map(|song| (song.title.as_str(), song.sources[0].label.clone())).collect()
    }

    #[test]
    fn the_preview_counts_what_would_match_and_what_would_not() {
        let library = library();
        let export = export();

        let matched = Matched::new(&library, &export).unwrap();

        assert_eq!(
            preview(&export, &matched),
            ElsewherePreview {
                playlists: 2,
                playlist_songs: 6,
                playlist_matched: 3,
                playlist_ambiguous: 1,
                plays: 8,
                plays_matched: 3,
                plays_ambiguous: 1,
                short_plays: 2,
                unnamed: 1,
            }
        );
    }

    #[test]
    fn playlists_keep_their_order_in_a_folder_and_plays_reach_the_songs_that_are_certain() {
        let mut library = library();
        let (summary, wanted) = import_again(&mut library, &mut PlaylistMapping::default(), true);

        assert_eq!((summary.playlists_created, summary.playlists_updated), (2, 0));
        assert_eq!((summary.playlist_songs, summary.playlist_matched, summary.playlist_ambiguous), (6, 3, 1));
        assert_eq!((summary.plays, summary.plays_matched, summary.plays_ambiguous, summary.plays_added), (8, 3, 1, 3));
        let folder = named(&library, "From Spotify");
        assert_eq!(folder.len(), 1);
        assert_eq!((folder[0].kind, folder[0].parent_id), (PlaylistKind::Folder, None));
        let trips = named(&library, "Road Trip");
        assert!(trips.iter().all(|trip| trip.parent_id == Some(folder[0].id)));
        // The same song twice stays twice; the two-album song and the song nobody owns are left out.
        assert_eq!(members(&library, trips[0].id), [1, 1]);
        assert_eq!(members(&library, trips[1].id), [2]);

        let (one, two, twin) = (library.track(1).unwrap().unwrap(), library.track(2).unwrap().unwrap(), library.track(4).unwrap().unwrap());
        assert_eq!((one.play_count, one.last_played), (2, Some(200)));
        assert_eq!(two.play_count, 0);
        assert_eq!((twin.play_count, twin.last_played), (1, Some(500)));
        assert_eq!(library.track(3).unwrap().unwrap().play_count, 0);

        // A song in a playlist is wanted once however often it is listed, and a song only the
        // history names is wanted once it has been played twice, not after one listen.
        assert_eq!(
            labelled(&wanted),
            [("Unowned Song", "Spotify playlist “Road Trip”".to_owned()), ("Heard Often", "Spotify listening history".to_owned())]
        );
        assert_eq!((wanted[0].sources[0].service.as_str(), wanted[1].album.as_str()), ("Spotify", "Elsewhere"));
    }

    #[test]
    fn importing_again_updates_in_place_and_counts_nothing_twice() {
        let saved = TempDir::new().unwrap();
        let path = saved.path().join("ImportedPlaylists.json");
        let mut library = library();
        import_again(&mut library, &mut PlaylistMapping::load(&path), true);
        let before = library.playlists().unwrap();

        let (again, wanted) = import_again(&mut library, &mut PlaylistMapping::load(&path), true);

        assert_eq!((again.playlists_created, again.playlists_updated, again.plays_matched, again.plays_added), (0, 2, 3, 0));
        assert_eq!(library.playlists().unwrap(), before);
        let trips = named(&library, "Road Trip");
        assert_eq!((members(&library, trips[0].id), members(&library, trips[1].id)), (vec![1, 1], vec![2]));
        assert_eq!(library.track(1).unwrap().unwrap().play_count, 2);
        assert_eq!(wanted.len(), 2);
    }

    #[test]
    fn a_playlist_is_the_same_playlist_whatever_its_capitals_underscores_and_punctuation() {
        let mut library = library();
        let mut mapping = PlaylistMapping::default();
        let called = |name: &str| Export {
            playlists: vec![Playlist { name: name.into(), songs: vec![song("Fixture Song", "The Fixtures", None)] }],
            ..Export::default()
        };

        import_export(&mut library, Service::Spotify, &called("Mr. Brightside Mix"), &mut mapping, false);
        let (again, _) = import_export(&mut library, Service::Spotify, &called("mr brightside mix"), &mut mapping, false);

        assert_eq!((again.playlists_created, again.playlists_updated), (0, 1));
        assert_eq!(named(&library, "Mr. Brightside Mix").len(), 1);
        assert_eq!(named(&library, "mr brightside mix").len(), 0);
    }

    #[test]
    fn a_deleted_playlist_is_made_again_and_the_rest_are_not() {
        let mut library = library();
        let mut mapping = PlaylistMapping::default();
        import_again(&mut library, &mut mapping, false);
        let second = named(&library, "Road Trip")[1].id;
        library.delete_playlist(second).unwrap();

        let (again, _) = import_again(&mut library, &mut mapping, false);

        assert_eq!((again.playlists_created, again.playlists_updated), (1, 1));
        assert_eq!(named(&library, "Road Trip").len(), 2);
    }

    #[test]
    fn the_id_of_a_deleted_playlist_is_not_given_to_the_next_one_so_nothing_else_is_filled() {
        let mut library = library();
        let mut mapping = PlaylistMapping::default();
        import_again(&mut library, &mut mapping, false);
        let second = named(&library, "Road Trip")[1].id;
        library.delete_playlist(second).unwrap();
        let mine = library.create_playlist("Mine", PlaylistKind::Regular, None).unwrap().id;
        assert!(mine > second);
        library.add_to_playlist(mine, &[3], None).unwrap();

        let (again, _) = import_again(&mut library, &mut mapping, false);

        assert_eq!((again.playlists_created, again.playlists_updated), (1, 1));
        assert_eq!(members(&library, mine), [3]);
        assert_eq!(named(&library, "Road Trip").len(), 2);
    }

    #[test]
    fn a_playlist_renamed_here_keeps_its_name_when_the_export_is_imported_again() {
        let mut library = library();
        let mut mapping = PlaylistMapping::default();
        import_again(&mut library, &mut mapping, false);
        let first = named(&library, "Road Trip")[0].id;
        library.rename_playlist(first, "Trip").unwrap();

        let (again, _) = import_again(&mut library, &mut mapping, false);

        assert_eq!((again.playlists_created, again.playlists_updated), (0, 2));
        assert_eq!(named(&library, "Trip").len(), 1);
        assert_eq!(members(&library, first), [1, 1]);
    }

    #[test]
    fn turning_plays_off_leaves_the_play_log_and_the_history_songs_alone() {
        let mut library = library();

        let (summary, wanted) = import_again(&mut library, &mut PlaylistMapping::default(), false);

        assert_eq!((summary.plays, summary.plays_matched, summary.plays_added), (0, 0, 0));
        assert!(library.recent_plays(0).unwrap().is_empty());
        assert_eq!(wanted.iter().map(|song| song.title.as_str()).collect::<Vec<_>>(), ["Unowned Song"]);
    }

    #[test]
    fn an_export_of_plays_alone_makes_no_folder() {
        let mut library = library();
        let plays = Export { plays: export().plays, ..Export::default() };

        import_export(&mut library, Service::ListenBrainz, &plays, &mut PlaylistMapping::default(), true);

        assert!(named(&library, "From ListenBrainz").is_empty());
        assert_eq!(library.track(1).unwrap().unwrap().play_count, 2);
    }

    #[test]
    fn another_service_recording_the_same_listens_adds_nothing() {
        let mut library = library();
        import_again(&mut library, &mut PlaylistMapping::default(), true);
        let plays = Export { plays: export().plays, ..Export::default() };

        let (summary, _) = import_export(&mut library, Service::ListenBrainz, &plays, &mut PlaylistMapping::default(), true);

        assert_eq!((summary.plays_matched, summary.plays_added), (3, 0));
        assert_eq!(library.track(1).unwrap().unwrap().play_count, 2);
    }

    #[test]
    fn a_long_history_goes_in_a_chunk_at_a_time_and_the_lcd_follows() {
        let mut library = library();
        let count = PLAYS_CHUNK * 2 + 100;
        let plays = (0..count).map(|n| play(n as i64 * 1_000, "Fixture Song", "The Fixtures", Some("Test Album"))).collect();
        let export = Export { plays, ..Export::default() };
        let matched = Matched::new(&library, &export).unwrap();
        let mut shown = Vec::new();
        let mut watch = |progress: ImportProgress| {
            if matches!(progress.stage, ImportStage::Plays) {
                shown.push((progress.done, progress.total));
            }
        };

        let (summary, _) =
            import(&mut library, Service::Spotify, &export, &matched, true, &mut PlaylistMapping::default(), &mut watch).unwrap();

        assert_eq!(summary.plays_added, count);
        assert_eq!(shown, [(0, Some(count)), (PLAYS_CHUNK, Some(count)), (PLAYS_CHUNK * 2, Some(count))]);
        assert_eq!(library.track(1).unwrap().unwrap().play_count, count as u32);
        let (again, _) =
            import(&mut library, Service::Spotify, &export, &matched, true, &mut PlaylistMapping::default(), &mut |_| {}).unwrap();
        assert_eq!(again.plays_added, 0);
    }

    #[test]
    fn what_the_sheet_matched_stands_until_a_track_it_found_has_gone() {
        let mut library = library();
        let matched = Matched::new(&library, &export()).unwrap();
        assert!(matched.still_valid(&library).unwrap());

        library.remove_tracks(&[2]).unwrap();

        assert!(!matched.still_valid(&library).unwrap());
        assert!(Matched::new(&library, &Export::default()).unwrap().still_valid(&library).unwrap());
    }

    #[test]
    fn a_stream_cannot_be_a_track_shorter_than_it_was_usually_played_and_is_then_not_known_to_be_missing() {
        let mut library = library();
        library.import_tracks(&[unsub_library::Track { duration_ms: 60_000, ..owned_song(5, "Short", "Sample Band", "Singles") }]).unwrap();
        let heard = |times: &[u64]| Export {
            plays: times.iter().map(|ms| Play { ms_played: Some(*ms), ..play(100, "Short", "Sample Band", None) }).collect(),
            ..Export::default()
        };

        let long = Matched::new(&library, &heard(&[200_000, 210_000])).unwrap();
        let short = Matched::new(&library, &heard(&[45_000, 45_000, 900_000])).unwrap();

        // Not found, but not missing either: nothing here says it isn't the song, so it isn't wanted.
        assert!(long.outcomes[long.plays[0]].is_ambiguous());
        // A few plays left running don't change what the song is usually heard for.
        assert_eq!(short.track(short.plays[0]), Some(5));
    }

    #[test]
    fn a_playlist_entry_is_not_held_to_how_long_the_same_song_was_played() {
        let mut library = library();
        library.import_tracks(&[unsub_library::Track { duration_ms: 60_000, ..owned_song(5, "Short", "Sample Band", "Singles") }]).unwrap();
        let export = Export {
            playlists: vec![Playlist { name: "Mix".into(), songs: vec![song("Short", "Sample Band", None)] }],
            plays: vec![Play { ms_played: Some(200_000), ..play(100, "Short", "Sample Band", None) }],
            ..Export::default()
        };

        let matched = Matched::new(&library, &export).unwrap();

        assert_eq!(matched.track(matched.playlists[0][0]), Some(5));
        assert!(matched.outcomes[matched.plays[0]].is_ambiguous());
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn a_zip_is_read_through_its_folder_without_its_other_files_and_a_hostile_one_is_refused() {
        let dir = TempDir::new().unwrap();
        let playlists =
            br#"{"playlists": [{"name": "Road Trip", "items": [{"track": {"trackName": "Fixture Song", "artistName": "The Fixtures"}}]}]}"#;
        let (good, hostile) = (dir.path().join("my_spotify_data.zip"), dir.path().join("other.zip"));
        write_zip(&good, &[("Spotify Account Data/Playlist1.json", playlists), ("Spotify Account Data/photo.jpg", &[7; 4_000])]);
        write_zip(&hostile, &[("../Playlist1.json", playlists)]);
        let mut totals = Vec::new();

        let export = read_export(Service::Spotify, &good, &mut |done, total| totals.push((done, total))).unwrap();

        assert_eq!(export.playlists.len(), 1);
        assert_eq!(export.playlists[0].songs[0].title, "Fixture Song");
        // Only the playlist file was unpacked, so only it was counted.
        assert_eq!(totals.last(), Some(&(playlists.len() as u64, playlists.len() as u64)));
        assert!(read_export(Service::Spotify, &hostile, &mut |_, _| {}).is_err());
        let loose = dir.path().join("loose");
        std::fs::create_dir(&loose).unwrap();
        std::fs::write(loose.join("Playlist1.json"), playlists).unwrap();
        assert_eq!(read_export(Service::Spotify, &loose, &mut |_, _| {}).unwrap().playlists.len(), 1);
    }
}
