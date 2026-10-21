//! Copy to Library: brings the songs on an iPod into the library with their details, play counts
//! and playlists. iTunes never let music back off an iPod. The iPod is only ever read: every
//! write goes to the library and its media folder.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use unsub_ipod::{Ipod, IpodTrack, Rescue, RescuePlaylist, RescueSong};
use unsub_library::{
    CopyIn, ExternalStats, ExternalTrack, Library, MatchOutcome, MatchQuality, PlaylistKind, Track, TrackOverrides, TrackQuery, song_key,
};

use crate::devices::{DevicesState, media_kind};
use crate::error::{CommandError, CommandResult};
use crate::import::{PlaylistMapping, merged, whole_stars};
use crate::library::{self, LibraryState};
use crate::sync::{SYNC_EVENT, SyncState};

/// The stage the LCD titles "Copying from …"; a sync's own stages are the iPod database's.
const STAGE: &str = "rescuing";
/// Songs copied under one hold of the writer, and the most bytes they add up to (unless one song is bigger). Each
/// is read off the iPod over USB and stopping waits for the chunk in hand, so a chunk of audiobooks must not be
/// a chunk of minutes. Between chunks the writer is let go for `COPY_BREATHER`.
const COPY_CHUNK: usize = 10;
const COPY_CHUNK_BYTES: u64 = 64 << 20;
/// A std mutex is not fair, so a thread that lets go and takes it again at once would be given it again every
/// time, and whoever waits (Get Info, a rating) would sleep through the whole copy.
const COPY_BREATHER: Duration = Duration::from_millis(5);
/// How far apart two lengths of one song can be when its title and artist both agree: what the matcher calls the
/// same recording.
const SAME_SONG_MS: u64 = 3_000;
/// How far apart they can be when only one name agrees. A sync writes the library's own length to the iPod, so a
/// song it put there matches to the millisecond, and another song of the same album or artist, whose number the
/// iPod remembers by mistake, does not.
const SYNCED_LENGTH_MS: u64 = 100;

/// The newest count of a sheet's choices. A count that finds it has been passed stops, since its choices have
/// changed and nobody is waiting for its answer.
static LATEST_PREVIEW: AtomicU64 = AtomicU64::new(0);

/// Which of the iPod's songs to copy.
#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RescueScope {
    All,
    /// The members of these playlists, by `RescuePlaylist::key`.
    Playlists {
        keys: Vec<String>,
    },
    /// Songs picked in the iPod's list, by the ids that list shows.
    Songs {
        ids: Vec<u32>,
    },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RescueOptions {
    /// Leave alone the songs the library already has, instead of copying them again.
    skip_known: bool,
    /// Merge the iPod's play counts, skips, ratings and dates into the songs.
    play_counts: bool,
    /// Make the iPod's playlists, in a folder named for it.
    playlists: bool,
    /// Restore the songs whose disk isn't connected from the iPod too, for a disk that is gone for good, instead of
    /// leaving them alone in case it is only unplugged.
    restore_offline: bool,
}

/// A song to bring into the library.
struct Todo<'a> {
    song: &'a RescueSong,
    /// The library song whose file is lost, which the iPod's copy takes the place of; `None` for a song the
    /// library doesn't have.
    restores: Option<i64>,
    /// Whether the iPod's counts are merged in. Not for the songs Unsub synced: the sync wrote the library's plays
    /// into the iPod's count, so they are the library's already.
    stats: bool,
}

/// What a copy will do, worked out before it starts.
#[derive(Default)]
struct Plan<'a> {
    /// The songs to bring in, in the iPod's order.
    todo: Vec<Todo<'a>>,
    /// Songs this app synced to the iPod, with the library songs they are.
    synced: Vec<(&'a RescueSong, i64)>,
    /// Songs the library already has, with its copy.
    found: Vec<(&'a RescueSong, i64)>,
    /// Songs asked for that are audio.
    songs: usize,
    /// Songs that are copy-protected, as far as their names say.
    protected: usize,
    /// Songs that fit more than one of the library's equally well, so nothing says which is meant.
    unsure: usize,
    /// Songs the database lists but whose file isn't on the iPod.
    missing: usize,
    /// Of the songs in `synced` and `found`, those whose file the library can't find because the disk it was on
    /// isn't connected, which says nothing about whether it is lost.
    offline: usize,
    /// The disks of all such songs, those restored as well, for the sheet to name.
    offline_volumes: BTreeSet<String>,
    /// Restore such songs after all.
    restore_offline: bool,
    /// Songs of the iPod that are copied although the library has one a few seconds off, which may be another cut.
    similar: HashSet<u32>,
    /// What the files to copy add up to.
    bytes: u64,
}

impl<'a> Plan<'a> {
    fn add(&mut self, todo: Todo<'a>) {
        if unsub_ipod::is_copy_protected(&todo.song.track) {
            self.protected += 1;
        } else {
            self.todo.push(todo);
        }
    }

    /// A song the library doesn't have.
    fn bring(&mut self, song: &'a RescueSong) {
        self.add(Todo { song, restores: None, stats: true });
    }

    /// A song the library has in `row`: left alone, unless the library has lost its file and the iPod has the
    /// song. A file missing because its disk isn't connected is not lost, unless the disk is said to be gone.
    fn have(&mut self, song: &'a RescueSong, row: &Track, stats: bool) {
        let lost = row.missing && song.bytes.is_some();
        let volume = lost.then(|| unplugged_volume(Path::new(&row.path))).flatten();
        let unplugged = volume.is_some();
        self.offline_volumes.extend(volume);
        if lost && (!unplugged || self.restore_offline) {
            self.add(Todo { song, restores: Some(row.id), stats });
            return;
        }
        self.offline += usize::from(unplugged);
        if stats {
            self.found.push((song, row.id));
        } else {
            self.synced.push((song, row.id));
        }
    }

    fn restored(&self) -> usize {
        self.todo.iter().filter(|todo| todo.restores.is_some()).count()
    }
}

fn playlists_to_make<'a>(rescue: &'a Rescue, scope: &RescueScope) -> Vec<&'a RescuePlaylist> {
    match scope {
        RescueScope::All => rescue.playlists.iter().collect(),
        RescueScope::Playlists { keys } => rescue.playlists.iter().filter(|playlist| keys.contains(&playlist.key)).collect(),
        RescueScope::Songs { .. } => Vec::new(),
    }
}

/// `None` is every song.
fn wanted(rescue: &Rescue, scope: &RescueScope) -> Option<HashSet<u32>> {
    match scope {
        RescueScope::All => None,
        RescueScope::Playlists { .. } => {
            Some(playlists_to_make(rescue, scope).into_iter().flat_map(|playlist| playlist.track_ids.iter().copied()).collect())
        }
        RescueScope::Songs { ids } => Some(ids.iter().copied().collect()),
    }
}

fn external(track: &IpodTrack) -> ExternalTrack {
    ExternalTrack {
        external_id: track.id.to_string(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album_artist: track.album_artist.clone(),
        album: track.album.clone(),
        duration_ms: Some(track.duration_ms),
        track_number: track.track_number,
        disc_number: track.disc_number,
        path: None,
        heard_ms: None,
    }
}

/// How the library looks for this song: by the database's details, or by the file's own tags when there is
/// no database to say.
fn described(rescue: &Rescue, song: &RescueSong) -> ExternalTrack {
    if rescue.from_files
        && let Ok(found) = ExternalTrack::from_file(song.track.id.to_string(), &song.track.path)
    {
        return found;
    }
    external(&song.track)
}

/// How the record of what was copied knows an iPod's song: by the database's persistent id, or for a song with
/// none by where its file is and how big. The name and tags don't come into it, which is the point: a song the
/// matcher can't tell from others (no artist, a title like "Track 1") is still the same song next time.
fn ledger_key(job: &Job, song: &RescueSong) -> String {
    let tag = format!("ipod:{}:track:", job.ipod.id);
    if song.track.dbid != 0 {
        return format!("{tag}{}", song.track.dbid);
    }
    let path = &song.track.path;
    let relative = path.strip_prefix(&job.ipod.mount_path).unwrap_or(path);
    format!("{tag}{}:{}", relative.to_string_lossy(), song.bytes.unwrap_or(0))
}

/// Whether two names are the same once normalized as the matcher normalizes them. Nothing is the same as nothing.
fn same_name(a: &str, b: &str, key: impl Fn(&str) -> String) -> bool {
    let a = key(a);
    a != key("") && a == key(b)
}

/// Whether a library row is the song the iPod has, as far as a mapped id can be believed. With the same title and
/// the same artist it can be a little different in length, as a recording is; with only one of them it must match
/// to the millisecond, since a neighbouring song of the album shares an artist and a length close by; and with no
/// length to go on (a placeholder, an unreadable file) it must have both. So a song retitled or given another
/// artist since the sync is still the same song, and another that reuses the id almost never is.
fn agrees(row: &Track, track: &IpodTrack) -> bool {
    let credited = |artist: &str, album_artist: &str| if artist.trim().is_empty() { album_artist.to_owned() } else { artist.to_owned() };
    let title = same_name(&row.title, &track.title, |title| song_key(title, ""));
    let artist =
        same_name(&credited(&row.artist, &row.album_artist), &credited(&track.artist, &track.album_artist), |artist| song_key("", artist));
    let length_known = row.duration_ms != 0 && track.duration_ms != 0;
    let gap = row.duration_ms.abs_diff(track.duration_ms);
    match (title, artist) {
        (true, true) => !length_known || gap <= SAME_SONG_MS,
        (true, false) | (false, true) => length_known && gap <= SYNCED_LENGTH_MS,
        (false, false) => false,
    }
}

/// The name of the volume a file is on if that volume isn't mounted: `/Volumes/<name>` with no such volume, or a
/// Windows drive that is not there. A file there is not gone, only out of reach, and may be fine when the disk is
/// plugged in again.
fn unplugged_volume(path: &Path) -> Option<String> {
    let mut parts = path.components();
    match parts.next()? {
        Component::Prefix(prefix) => {
            let root: PathBuf = path.components().take(2).collect();
            (!root.exists()).then(|| prefix.as_os_str().to_string_lossy().into_owned())
        }
        Component::RootDir => match (parts.next(), parts.next()) {
            (Some(Component::Normal(top)), Some(Component::Normal(name))) if top == "Volumes" => {
                (!Path::new("/Volumes").join(name).exists()).then(|| name.to_string_lossy().into_owned())
            }
            _ => None,
        },
        _ => None,
    }
}

/// The library's rows for these ids, those it still has.
fn rows_of(library: &Library, ids: &[i64]) -> unsub_library::Result<HashMap<i64, Track>> {
    let query = TrackQuery { kind: None, ids: Some(ids.to_vec()), ..TrackQuery::default() };
    Ok(library.tracks(&query)?.into_iter().map(|track| (track.id, track)).collect())
}

/// Works out what a copy would do. The library is looked at in two short holds of its lock, and the files are
/// not opened under either: `stop` is asked between files, and `None` means it said so.
fn make_plan<'a>(library: &LibraryState, job: &Job<'a>, stop: &dyn Fn() -> bool) -> CommandResult<Option<Plan<'a>>> {
    let (rescue, options) = (job.rescue, job.options);
    let wanted = wanted(rescue, job.scope);
    let asked_for: Vec<&RescueSong> = rescue
        .songs
        .iter()
        .filter(|song| media_kind(&song.track.media_kind).is_some() && wanted.as_ref().is_none_or(|ids| ids.contains(&song.track.id)))
        .collect();
    let mut plan = Plan { songs: asked_for.len(), restore_offline: options.restore_offline, ..Plan::default() };

    // What was copied off this iPod before, which `land` records with the songs, so every id here is a song the
    // library has. Copying everything again is asked for by not skipping, and the record has no say then.
    let keys: Vec<String> = if options.skip_known { asked_for.iter().map(|song| ledger_key(job, song)).collect() } else { Vec::new() };
    let (ledger, rows) = {
        let queries = library.queries();
        let ledger = queries.rescued(&keys)?;
        let remembered = |song: &RescueSong| ledger.get(&ledger_key(job, song)).copied();
        let claimed: Vec<i64> = asked_for.iter().flat_map(|song| [song.library_id, remembered(song)]).flatten().collect();
        let rows = rows_of(&queries, &claimed)?;
        (ledger, rows)
    };
    let remembered = |song: &RescueSong| ledger.get(&ledger_key(job, song)).copied();
    let mut undecided = Vec::new();
    for song in asked_for {
        // Unsub.json's ids are believed only as far as the library bears them out: after a rebuilt or lost library
        // an id can be gone, or be another song's.
        let synced = song.library_id.and_then(|id| rows.get(&id)).filter(|row| agrees(row, &song.track));
        if let Some(row) = synced {
            plan.have(song, row, false);
        } else if let Some(row) = remembered(song).and_then(|id| rows.get(&id)) {
            plan.have(song, row, true);
        } else if song.bytes.is_none() {
            plan.missing += 1;
        } else {
            undecided.push(song);
        }
    }

    let mut externals = Vec::with_capacity(undecided.len());
    for song in &undecided {
        if stop() {
            return Ok(None);
        }
        externals.push(described(rescue, song));
    }
    if stop() {
        return Ok(None);
    }
    let (outcomes, matched) = {
        let queries = library.queries();
        let outcomes = queries.match_external_strict(&externals)?;
        let ids: Vec<i64> = outcomes.iter().filter_map(MatchOutcome::track_id).collect();
        (outcomes, rows_of(&queries, &ids)?)
    };
    for (song, outcome) in undecided.into_iter().zip(outcomes) {
        match outcome {
            // Only a sure match counts, as in the Music import.
            MatchOutcome::Found(found) if options.skip_known && found.quality != MatchQuality::Close => {
                plan.have(song, &matched[&found.track_id], true);
            }
            MatchOutcome::Ambiguous if options.skip_known => plan.unsure += 1,
            outcome => {
                // A version a few seconds off may be another recording, and leaving it on the iPod loses it. Same-CD
                // encodes land within the matcher's tolerance, so these are the rarer case.
                if options.skip_known && matches!(outcome, MatchOutcome::Found(_)) {
                    plan.similar.insert(song.track.id);
                }
                plan.bring(song);
            }
        }
    }
    plan.bytes = plan.todo.iter().filter_map(|todo| todo.song.bytes).fold(0, u64::saturating_add);
    Ok(Some(plan))
}

/// The iPod's own details for a song, pinned over whatever its file says: users edited them in iTunes. A field
/// the iPod has nothing for stays `None` rather than blanking the file's.
fn overrides(track: &IpodTrack) -> TrackOverrides {
    let text = |value: &str| (!value.trim().is_empty()).then(|| value.to_owned());
    TrackOverrides {
        kind: media_kind(&track.media_kind),
        title: text(&track.title),
        artist: text(&track.artist),
        album: text(&track.album),
        album_artist: text(&track.album_artist),
        genre: text(&track.genre),
        composer: text(&track.composer),
        date_added: track.date_added,
        year: track.year,
        track_number: track.track_number,
        track_count: track.track_count,
        disc_number: track.disc_number,
        disc_count: track.disc_count,
        compilation: Some(track.compilation),
        ..TrackOverrides::default()
    }
}

/// With no database the files' own tags are all there is.
fn details(rescue: &Rescue, song: &RescueSong) -> TrackOverrides {
    if rescue.from_files { TrackOverrides::default() } else { overrides(&song.track) }
}

fn stats_of(track: &IpodTrack, enabled: bool) -> ExternalStats {
    ExternalStats {
        play_count: track.play_count,
        skip_count: track.skip_count,
        last_played: track.last_played,
        last_skipped: None,
        rating: whole_stars(track.rating),
        date_added: track.date_added,
        enabled,
    }
}

/// Merges what the iPod recorded into the library's songs, as the Music import does: counts and dates keep the
/// larger or later value and a rating fills in only where there is none, so doing it again changes nothing, and
/// plays made in Unsub since are on top of the iPod's. A song's checkbox is left as the library has it, since an
/// iPod has none.
fn bring_stats(library: &mut Library, songs: &[(&RescueSong, i64)]) -> unsub_library::Result<()> {
    let ids: Vec<i64> = songs.iter().map(|(_, id)| *id).collect();
    let rows = rows_of(library, &ids)?;
    // One file can be listed twice on an iPod, and combining their stats the same way in any order keeps the
    // result from depending on which came last.
    let mut stats: HashMap<i64, ExternalStats> = HashMap::new();
    for (song, id) in songs {
        if let Some(row) = rows.get(id) {
            let theirs = stats_of(&song.track, row.enabled);
            stats.entry(*id).and_modify(|earlier| *earlier = merged(earlier.clone(), theirs.clone())).or_insert(theirs);
        }
    }
    library.apply_external_stats(&stats.into_iter().collect::<Vec<_>>())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Summary {
    /// Songs new to the library.
    copied: usize,
    /// Songs the library listed but had lost the files of, which now have them again.
    restored: usize,
    /// Of the copied, those that look like another version of a song the library has.
    similar: usize,
    /// Songs the library already had.
    known: usize,
    /// Songs the library has no file for because their disk isn't connected, which are left as they are.
    offline: usize,
    protected: usize,
    unsure: usize,
    /// Songs whose file isn't on the iPod, and files the library couldn't take.
    failed: usize,
    stopped: bool,
    /// The iPod went away part-way.
    disconnected: bool,
    playlists_created: usize,
    playlists_updated: usize,
}

struct Job<'a> {
    ipod: &'a Ipod,
    rescue: &'a Rescue,
    scope: &'a RescueScope,
    options: RescueOptions,
    /// Where the playlists made from each iPod are recorded, so copying again updates them.
    mapping_path: &'a Path,
}

/// What the LCD shows for a song while it is copied.
fn label(song: &RescueSong) -> String {
    if !song.track.title.is_empty() {
        return song.track.title.clone();
    }
    song.track.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The songs to take under one hold of the writer: up to `COPY_CHUNK` of them, adding up to no more than
/// `COPY_CHUNK_BYTES`, but always at least one.
fn next_chunk<'a, 'b>(todo: &'b [Todo<'a>]) -> &'b [Todo<'a>] {
    let mut bytes = 0u64;
    let mut count = 0;
    for item in todo.iter().take(COPY_CHUNK) {
        let size = item.song.bytes.unwrap_or(0);
        if count > 0 && bytes.saturating_add(size) > COPY_CHUNK_BYTES {
            break;
        }
        bytes = bytes.saturating_add(size);
        count += 1;
    }
    &todo[..count]
}

/// What became of one song of a chunk.
enum Landed {
    /// Copied in as a new song, with this id.
    New(i64),
    /// A song the library had lost the file of, which now has the iPod's.
    Restored(i64),
    /// A song whose own file turned out to be there after all, so nothing was done.
    Present(i64),
    Failed,
}

impl Landed {
    fn id(&self) -> Option<i64> {
        match self {
            Self::New(id) | Self::Restored(id) | Self::Present(id) => Some(*id),
            Self::Failed => None,
        }
    }
}

/// Brings a chunk of songs into the library: the new ones copied in, the lost ones' files restored, each recorded
/// as copied, then their stats merged, all under the one hold of the writer, so a stop can't land between
/// a file and its record. A new song's record goes in the same transaction as the song.
fn land(writer: &mut Library, job: &Job, chunk: &[&Todo]) -> unsub_library::Result<Vec<Landed>> {
    let new: Vec<usize> = (0..chunk.len()).filter(|&at| chunk[at].restores.is_none()).collect();
    let files: Vec<CopyIn> = new
        .iter()
        .map(|&at| {
            let song = chunk[at].song;
            CopyIn { path: song.track.path.clone(), overrides: details(job.rescue, song), source_key: ledger_key(job, song) }
        })
        .collect();
    let added = writer.copy_files_in(&files)?;
    let mut landed: Vec<Landed> = chunk.iter().map(|_| Landed::Failed).collect();
    for (&at, id) in new.iter().zip(added.ids) {
        if let Some(id) = id {
            landed[at] = Landed::New(id);
        }
    }
    for (todo, outcome) in chunk.iter().zip(&mut landed) {
        if let Some(lost) = todo.restores {
            match writer.restore_from(lost, &todo.song.track.path, &ledger_key(job, todo.song)) {
                Ok(true) => *outcome = Landed::Restored(lost),
                Ok(false) => *outcome = Landed::Present(lost),
                Err(error @ unsub_library::Error::Database(_)) => return Err(error),
                // This song's file wouldn't copy; the rest go on.
                Err(_) => {}
            }
        }
    }
    if job.options.play_counts {
        let counted: Vec<(&RescueSong, i64)> = chunk
            .iter()
            .zip(&landed)
            .filter(|(todo, _)| todo.stats)
            .filter_map(|(todo, outcome)| Some((todo.song, outcome.id()?)))
            .collect();
        bring_stats(writer, &counted)?;
    }
    Ok(landed)
}

/// Makes the iPod's playlists in a folder named for it. Each is found by the iPod's own id through `mapping`
/// and refilled rather than made again, and its songs are whichever the library now has.
fn recreate_playlists(
    library: &mut Library,
    mapping: &mut PlaylistMapping,
    ipod: &Ipod,
    playlists: &[&RescuePlaylist],
    ours: &HashMap<u32, i64>,
    summary: &mut Summary,
) -> unsub_library::Result<()> {
    let existing = library.playlists()?;
    let tag = format!("ipod:{}:", ipod.id);
    let name = format!("From {}", ipod.name);
    let (folder, created) = mapping.place(library, &existing, tag.clone(), &name, PlaylistKind::Folder, None)?;
    if !created {
        library.rename_playlist(folder, &name)?;
    }
    // A song deleted from the library since it was found can't be put in a playlist.
    let present = rows_of(library, &ours.values().copied().collect::<Vec<_>>())?;
    for playlist in playlists {
        let (id, created) =
            mapping.place(library, &existing, format!("{tag}{}", playlist.key), &playlist.name, PlaylistKind::Regular, Some(folder))?;
        if created {
            summary.playlists_created += 1;
        } else {
            library.rename_playlist(id, &playlist.name)?;
            summary.playlists_updated += 1;
        }
        let members: Vec<i64> =
            playlist.track_ids.iter().filter_map(|song| ours.get(song).copied()).filter(|track| present.contains_key(track)).collect();
        library.set_playlist_items(id, &members)?;
    }
    Ok(())
}

/// Copies what the plan says into the library, a few songs at a time, then makes the playlists. Stopping leaves
/// every song copied so far whole and in the library, and copying again carries on from there: the songs it finds
/// there are the library's now, and the record of what was copied covers the ones it can't tell apart.
fn copy(library: &LibraryState, job: &Job, cancel: &AtomicBool, progress: &mut dyn FnMut(usize, usize, &str)) -> CommandResult<Summary> {
    let stopping = || cancel.load(Ordering::Relaxed);
    let Some(plan) = make_plan(library, job, &stopping)? else {
        return Ok(Summary { stopped: true, ..Summary::default() });
    };
    let folder = library.queries().media_settings()?.folder.ok_or(unsub_library::Error::NoMediaFolder)?;
    unsub_ipod::ensure_room(plan.bytes, &folder).map_err(|error| CommandError::from(error.to_string()))?;
    let mut summary = Summary {
        known: plan.synced.len() + plan.found.len() - plan.offline,
        offline: plan.offline,
        protected: plan.protected,
        unsure: plan.unsure,
        failed: plan.missing,
        ..Summary::default()
    };

    if job.options.play_counts && !plan.found.is_empty() {
        library.with_writer(|writer| bring_stats(writer, &plan.found))?;
    }
    let mut landed: Vec<(&RescueSong, i64)> = Vec::new();
    let total = plan.todo.len();
    let mut rest = plan.todo.as_slice();
    while !rest.is_empty() {
        if stopping() {
            summary.stopped = true;
            break;
        }
        let chunk = next_chunk(rest);
        progress(total - rest.len(), total, &label(chunk[0].song));
        rest = &rest[chunk.len()..];
        // A protected file with an innocent name is only found by opening it, which is left to now, for the songs
        // about to be copied, and done before the writer is taken.
        let (chunk, protected): (Vec<&Todo>, Vec<&Todo>) =
            chunk.iter().partition(|todo| !unsub_ipod::holds_protected_audio(&todo.song.track));
        summary.protected += protected.len();
        let outcomes = library.with_writer(|writer| land(writer, job, &chunk))?;
        // An iPod pulled out part-way fails every song after it, which says nothing about the songs.
        let gone = !job.ipod.mount_path.is_dir();
        for (todo, outcome) in chunk.iter().zip(outcomes) {
            if let Some(id) = outcome.id() {
                landed.push((todo.song, id));
            }
            match outcome {
                Landed::New(_) => {
                    summary.copied += 1;
                    summary.similar += usize::from(plan.similar.contains(&todo.song.track.id));
                }
                Landed::Restored(_) => summary.restored += 1,
                Landed::Present(_) => summary.known += 1,
                Landed::Failed if !gone => summary.failed += 1,
                Landed::Failed => {}
            }
        }
        if gone {
            summary.disconnected = true;
            break;
        }
        std::thread::sleep(COPY_BREATHER);
    }

    let to_make = playlists_to_make(job.rescue, job.scope);
    if job.options.playlists && !summary.stopped && !summary.disconnected && !to_make.is_empty() {
        let ours: HashMap<u32, i64> = plan.synced.iter().chain(&plan.found).chain(&landed).map(|(song, id)| (song.track.id, *id)).collect();
        library.with_writer(|writer| {
            // Read under the writer's lock, so no other writer has an older copy to save over this one.
            let mut mapping = PlaylistMapping::load(job.mapping_path);
            recreate_playlists(writer, &mut mapping, job.ipod, &to_make, &ours, &mut summary)
        })?;
    }
    Ok(summary)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RescuePreview {
    /// Audio songs asked for.
    songs: usize,
    /// Songs that would be copied.
    new: usize,
    /// Songs the library lists but has lost the files of, which would be restored.
    restored: usize,
    /// Of the new, those that look like another version of a song the library has.
    similar: usize,
    /// Songs the library already has.
    known: usize,
    /// Songs the library lists, with no file it can find because the disk they were on isn't connected, which are
    /// left alone: the disk may only be unplugged. None if they are to be restored anyway.
    offline: usize,
    /// The disks of those songs, whether they are left alone or restored.
    offline_volumes: Vec<String>,
    protected: usize,
    unsure: usize,
    /// Songs whose file isn't on the iPod.
    missing: usize,
    bytes: u64,
    free_bytes: u64,
    fits: bool,
    /// The playlists the sheet can offer, whatever the choices.
    playlists: Vec<RescuePlaylistSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RescuePlaylistSummary {
    key: String,
    name: String,
    on_the_go: bool,
    smart: bool,
    songs: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RescueStatus {
    device_id: String,
    stage: &'static str,
    done: usize,
    total: usize,
    current: String,
}

fn to_error(error: impl std::fmt::Display) -> CommandError {
    CommandError::from(error.to_string())
}

fn read(app: &AppHandle, device_id: &str) -> CommandResult<(Ipod, Rescue, PathBuf)> {
    let ipod = app.state::<DevicesState>().ipod(device_id)?;
    let rescue = unsub_ipod::read_rescue(&ipod).map_err(to_error)?;
    let mapping_path = app.path().app_data_dir().map_err(to_error)?.join("ImportedPlaylists.json");
    Ok((ipod, rescue, mapping_path))
}

fn preview(app: &AppHandle, device_id: &str, scope: &RescueScope, options: RescueOptions) -> CommandResult<RescuePreview> {
    let mine = LATEST_PREVIEW.fetch_add(1, Ordering::SeqCst) + 1;
    let (ipod, rescue, mapping_path) = read(app, device_id)?;
    let job = Job { ipod: &ipod, rescue: &rescue, scope, options, mapping_path: &mapping_path };
    let library = app.state::<LibraryState>();
    let passed = || LATEST_PREVIEW.load(Ordering::SeqCst) != mine;
    let plan = make_plan(&library, &job, &passed)?.ok_or_else(|| CommandError::from("A newer count took its place.".to_owned()))?;
    let folder = library.queries().media_settings()?.folder.ok_or(unsub_library::Error::NoMediaFolder)?;
    let free_bytes = unsub_ipod::free_space(&folder).map_err(to_error)?;
    let playlists = rescue
        .playlists
        .iter()
        .map(|playlist| RescuePlaylistSummary {
            key: playlist.key.clone(),
            name: playlist.name.clone(),
            on_the_go: playlist.on_the_go,
            smart: playlist.smart,
            songs: playlist.track_ids.len(),
        })
        .collect();
    Ok(RescuePreview {
        songs: plan.songs,
        new: plan.todo.len() - plan.restored(),
        restored: plan.restored(),
        similar: plan.similar.len(),
        known: plan.synced.len() + plan.found.len() - plan.offline,
        offline: plan.offline,
        offline_volumes: plan.offline_volumes.iter().cloned().collect(),
        protected: plan.protected,
        unsure: plan.unsure,
        missing: plan.missing,
        bytes: plan.bytes,
        free_bytes,
        fits: plan.bytes <= free_bytes,
        playlists,
    })
}

/// What Copy to Library would do with these choices; touches nothing. Async because it reads the iPod's database
/// and every file's size, and matches its songs against the library.
#[tauri::command]
pub async fn device_rescue_preview(
    app: AppHandle,
    device_id: String,
    scope: RescueScope,
    options: RescueOptions,
) -> CommandResult<RescuePreview> {
    tauri::async_runtime::spawn_blocking(move || preview(&app, &device_id, &scope, options)).await.map_err(to_error)?
}

fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn songs(count: usize) -> String {
    format!("{} {}", grouped(count), if count == 1 { "song" } else { "songs" })
}

/// "was" and "is", or "were" and "are", to go with a count.
fn verbs(count: usize) -> (&'static str, &'static str) {
    if count == 1 { ("was", "is") } else { ("were", "are") }
}

fn notice(summary: &Summary, ipod_name: &str) -> String {
    let landed = summary.copied + summary.restored;
    if summary.disconnected {
        return format!("The iPod was disconnected. {} {} copied.", songs(landed), verbs(landed).0);
    }
    let mut sentences = Vec::new();
    if summary.stopped {
        sentences.push(format!("Copy stopped. {} copied from {ipod_name}.", songs(landed)));
    } else if landed == 0 {
        sentences.push(format!("Nothing new to copy from {ipod_name}."));
    } else if summary.copied > 0 {
        sentences.push(format!("Copied {} from {ipod_name}.", songs(summary.copied)));
    }
    if summary.restored > 0 && !summary.stopped {
        let (file, was) = if summary.restored == 1 { ("file", "was") } else { ("files", "were") };
        sentences.push(format!("Restored {} whose {file} {was} missing from your library.", songs(summary.restored)));
    }
    if summary.similar > 0 {
        let (looks, was) = if summary.similar == 1 { ("looks like another version", "was") } else { ("look like other versions", "were") };
        sentences.push(format!("{} {looks} of songs you have and {was} copied too.", grouped(summary.similar)));
    }
    if summary.known > 0 {
        sentences.push(format!("{} {} already in your library.", grouped(summary.known), verbs(summary.known).0));
    }
    if summary.offline > 0 {
        let (was, is) = verbs(summary.offline);
        sentences.push(format!("{} {is} on a disk that isn't connected and {was} left alone.", grouped(summary.offline)));
    }
    if summary.protected > 0 {
        let (was, is) = verbs(summary.protected);
        sentences.push(format!("{} {is} copy-protected and {was} left out.", grouped(summary.protected)));
    }
    if summary.unsure > 0 {
        let left_out = if summary.unsure == 1 { "it was" } else { "they were" };
        sentences.push(format!("{} could be more than one of your songs, so {left_out} left out.", grouped(summary.unsure)));
    }
    if summary.failed > 0 {
        sentences.push(format!("{} couldn't be copied.", grouped(summary.failed)));
    }
    let playlists = summary.playlists_created + summary.playlists_updated;
    if playlists > 0 {
        let noun = if playlists == 1 { "playlist" } else { "playlists" };
        sentences.push(format!("{} {noun} {} in “From {ipod_name}”.", grouped(playlists), verbs(playlists).1));
    }
    sentences.join(" ")
}

fn run(
    app: &AppHandle,
    device_id: &str,
    scope: &RescueScope,
    options: RescueOptions,
    cancel: &AtomicBool,
) -> CommandResult<(Summary, String)> {
    let mut status = |done: usize, total: usize, current: &str| {
        let status = RescueStatus { device_id: device_id.to_owned(), stage: STAGE, done, total, current: current.to_owned() };
        let _ = app.emit(SYNC_EVENT, Some(status));
    };
    // Reading a big iPod takes a while, and the LCD should say so from the first moment.
    status(0, 0, "Reading the iPod…");
    let (ipod, rescue, mapping_path) = read(app, device_id)?;
    let job = Job { ipod: &ipod, rescue: &rescue, scope, options, mapping_path: &mapping_path };
    let summary = copy(&app.state::<LibraryState>(), &job, cancel, &mut status)?;
    Ok((summary, ipod.name.clone()))
}

/// Copy to Library: copies the chosen songs off the iPod in the background. It takes the sync's place, so the
/// two can't run at once, and the LCD shows it and stops it the same way.
#[tauri::command]
pub fn device_rescue(
    app: AppHandle,
    state: State<'_, SyncState>,
    device_id: String,
    scope: RescueScope,
    options: RescueOptions,
) -> CommandResult<()> {
    let cancel = state.begin(&device_id)?;
    std::thread::spawn(move || {
        let result = run(&app, &device_id, &scope, options, &cancel);
        app.state::<SyncState>().finish();
        let _ = app.emit(SYNC_EVENT, None::<RescueStatus>);
        let _ = app.emit(library::CHANGED_EVENT, ());
        let message = match result {
            Ok((summary, name)) => notice(&summary, &name),
            Err(error) => format!("Copy failed: {error}"),
        };
        let _ = app.emit(crate::playback::PLAYER_EVENT, unsub_playback::PlayerEvent::Notice { message });
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::atomic::AtomicBool;

    use tempfile::TempDir;
    use unsub_ipod::{DatabaseFormat, IpodMediaKind, Prepared, SyncPlaylist, SyncSupport, SyncTrack};
    use unsub_library::{SortKey, Track};

    use super::*;

    const EVERYTHING: RescueOptions = RescueOptions { skip_known: true, play_counts: true, playlists: true, restore_offline: false };

    struct World {
        _root: TempDir,
        library: LibraryState,
        media: PathBuf,
        ipod_dir: PathBuf,
        mapping: PathBuf,
    }

    fn world() -> World {
        let root = tempfile::tempdir().unwrap();
        let media = root.path().join("media");
        let library = LibraryState::open(root.path().join("Library.sqlite"), root.path().join("Artwork")).unwrap();
        library.with_writer(|writer| writer.set_media_folder(&media)).unwrap();
        let ipod_dir = root.path().join("ipod");
        fs::create_dir_all(&ipod_dir).unwrap();
        World { mapping: root.path().join("ImportedPlaylists.json"), media, library, ipod_dir, _root: root }
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/library/tests/fixtures").join(name)
    }

    /// A song on the iPod: a copy of a fixture in its music folder, listed as `details` say.
    fn on_ipod(world: &World, id: u32, file: &str, details: IpodTrack) -> RescueSong {
        let extension = Path::new(file).extension().unwrap().to_str().unwrap();
        let path = world.ipod_dir.join(format!("iPod_Control/Music/F00/S{id:03}.{extension}"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy(fixture(file), &path).unwrap();
        let bytes = fs::metadata(&path).unwrap().len();
        RescueSong { track: IpodTrack { id, path, file_size: bytes, ..details }, library_id: None, bytes: Some(bytes) }
    }

    fn tagged(title: &str) -> IpodTrack {
        IpodTrack {
            title: title.into(),
            artist: "Beyoncé".into(),
            album_artist: "Beyoncé".into(),
            album: "I Am".into(),
            duration_ms: 500,
            ..IpodTrack::default()
        }
    }

    fn ipod(world: &World, name: &str) -> Ipod {
        Ipod {
            id: "000A27001A2B3C4D".into(),
            name: name.into(),
            mount_path: world.ipod_dir.clone(),
            model: None,
            capacity_bytes: 0,
            free_bytes: 0,
            firewire_guid: None,
            guid_from_usb: false,
            serial_number: None,
            database: DatabaseFormat::ITunesDb,
            sync: SyncSupport::Supported,
        }
    }

    fn playlist(key: &str, name: &str, songs: &[u32]) -> RescuePlaylist {
        RescuePlaylist { key: key.into(), name: name.into(), on_the_go: false, smart: false, track_ids: songs.to_vec() }
    }

    fn rescue(songs: Vec<RescueSong>, playlists: Vec<RescuePlaylist>) -> Rescue {
        Rescue { songs, playlists, from_files: false }
    }

    fn try_copy(world: &World, ipod: &Ipod, rescue: &Rescue, scope: &RescueScope, options: RescueOptions) -> CommandResult<Summary> {
        let job = Job { ipod, rescue, scope, options, mapping_path: &world.mapping };
        copy(&world.library, &job, &AtomicBool::new(false), &mut |_, _, _| {})
    }

    fn copy_all(world: &World, ipod: &Ipod, rescue: &Rescue) -> Summary {
        try_copy(world, ipod, rescue, &RescueScope::All, EVERYTHING).unwrap()
    }

    fn library_tracks(world: &World) -> Vec<Track> {
        world.library.queries().tracks(&TrackQuery { kind: None, sort: SortKey::Name, ..TrackQuery::default() }).unwrap()
    }

    /// The playlists a copy made, without the smart ones every library starts with.
    fn made_playlists(world: &World) -> Vec<unsub_library::Playlist> {
        world.library.queries().playlists().unwrap().into_iter().filter(|playlist| playlist.kind != PlaylistKind::Smart).collect()
    }

    fn by_title(world: &World, title: &str) -> Track {
        library_tracks(world).into_iter().find(|track| track.title == title).unwrap_or_else(|| panic!("no song titled {title}"))
    }

    /// Every file under `dir` with its bytes.
    fn contents(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else {
                    found.insert(path.clone(), fs::read(&path).unwrap());
                }
            }
        }
        let mut found = BTreeMap::new();
        walk(dir, &mut found);
        found
    }

    fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut atom = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        atom.extend_from_slice(kind);
        atom.extend_from_slice(body);
        atom
    }

    /// An MPEG-4 audio file whose only sample description is `entry`.
    fn mp4_with_entry(entry: &[u8; 4]) -> Vec<u8> {
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend_from_slice(&36u32.to_be_bytes());
        stsd.extend_from_slice(entry);
        let stbl = atom(b"stbl", &atom(b"stsd", &stsd));
        let moov = atom(b"moov", &atom(b"trak", &atom(b"mdia", &atom(b"minf", &stbl))));
        [atom(b"ftyp", b"M4A \0\0\0\0"), atom(b"mdat", &[1, 2, 3]), moov].concat()
    }

    fn files_in_media(world: &World) -> Vec<PathBuf> {
        contents(&world.media).into_keys().map(|path| path.strip_prefix(&world.media).unwrap().to_owned()).collect()
    }

    fn in_library(world: &World, fixture_name: &str) -> i64 {
        let own = world._root.path().join(format!("mine-{fixture_name}"));
        fs::copy(fixture(fixture_name), &own).unwrap();
        world.library.with_writer(|writer| writer.add_files_in_place(&[(own, TrackOverrides::default())])).unwrap().ids[0].unwrap()
    }

    #[test]
    fn new_songs_are_copied_with_the_ipods_details_and_stats_and_the_ipod_is_only_read() {
        let w = world();
        let details = IpodTrack {
            genre: "Pop".into(),
            album: "Single".into(),
            year: Some(2008),
            track_number: Some(1),
            track_count: Some(2),
            disc_number: Some(1),
            disc_count: Some(1),
            play_count: 12,
            skip_count: 2,
            last_played: Some(1_700_000_000),
            rating: 80,
            date_added: Some(1_200_000_000),
            ..tagged("Halo (iTunes Edit)")
        };
        let song = on_ipod(&w, 1, "halo.mp3", details);
        let before = contents(&w.ipod_dir);

        let summary = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![song.clone()], vec![]));

        assert_eq!((summary.copied, summary.known, summary.failed), (1, 0, 0));
        let tracks = library_tracks(&w);
        assert_eq!(tracks.len(), 1);
        let copied = &tracks[0];
        assert_eq!((copied.title.as_str(), copied.album.as_str(), copied.genre.as_str()), ("Halo (iTunes Edit)", "Single", "Pop"));
        assert_eq!((copied.year, copied.track_number, copied.track_count, copied.disc_number), (Some(2008), Some(1), Some(2), Some(1)));
        // The file says it is from a compilation; the iPod, which users edited in iTunes, says it is not.
        assert!(!copied.compilation);
        assert_eq!((copied.play_count, copied.skip_count, copied.rating), (12, 2, 80));
        assert_eq!((copied.last_played, copied.date_added), (Some(1_700_000_000), 1_200_000_000));
        let media = fs::canonicalize(&w.media).unwrap();
        assert_eq!(Path::new(&copied.path), media.join("Beyoncé/Single/01 Halo (iTunes Edit).mp3"));
        assert_eq!(files_in_media(&w), [Path::new("Beyoncé/Single/01 Halo (iTunes Edit).mp3")]);
        assert_eq!(contents(&w.ipod_dir), before);
    }

    #[test]
    fn a_song_the_library_has_is_not_copied_but_its_counts_are_merged_and_its_checkbox_kept() {
        let w = world();
        let id = in_library(&w, "halo.mp3");
        w.library
            .with_writer(|writer| {
                for at in [100, 200, 300] {
                    writer.record_play(id, at)?;
                }
                writer.set_enabled(&[id], false)
            })
            .unwrap();
        let song = on_ipod(&w, 1, "halo.mp3", IpodTrack { play_count: 7, rating: 60, ..tagged("Halo") });
        let rescue = rescue(vec![song], vec![]);

        let summary = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue);

        assert_eq!((summary.copied, summary.known), (0, 1));
        assert!(files_in_media(&w).is_empty());
        let halo = by_title(&w, "Halo");
        assert_eq!(halo.id, id);
        // The three plays made here are ones the iPod never saw, so they are on top of its seven.
        assert_eq!((halo.play_count, halo.rating, halo.last_played, halo.enabled), (10, 60, Some(300), false));

        copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue);
        assert_eq!(by_title(&w, "Halo").play_count, 10);
    }

    #[test]
    fn a_song_that_does_not_skip_known_ones_is_copied_again_as_a_new_one() {
        let w = world();
        in_library(&w, "halo.mp3");
        let song = on_ipod(&w, 1, "halo.mp3", tagged("Halo"));
        let options = RescueOptions { skip_known: false, ..EVERYTHING };

        let summary = try_copy(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![song], vec![]), &RescueScope::All, options).unwrap();

        assert_eq!((summary.copied, summary.known), (1, 0));
        assert_eq!(library_tracks(&w).len(), 2);
    }

    #[test]
    fn songs_the_app_synced_are_in_the_library_and_their_counts_are_left_to_the_sync() {
        let w = world();
        let id = in_library(&w, "halo.mp3");
        let mut song = on_ipod(&w, 1, "halo.mp3", IpodTrack { play_count: 9, ..tagged("Halo") });
        song.library_id = Some(id);

        let summary = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![song], vec![]));

        assert_eq!((summary.copied, summary.known), (0, 1));
        assert_eq!(library_tracks(&w).len(), 1);
        assert_eq!(by_title(&w, "Halo").play_count, 0);
        assert!(files_in_media(&w).is_empty());
    }

    #[test]
    fn unsub_json_is_believed_only_as_far_as_the_library_bears_it_out() {
        let w = world();
        // A rebuilt library: the id the iPod remembers is gone, so the only copy of the song is the iPod's.
        let mut gone = on_ipod(&w, 1, "halo.mp3", tagged("Halo"));
        gone.library_id = Some(41);
        // A reused id: the library's song with that number is another song entirely.
        let other = in_library(&w, "zombie.ogg");
        let mut reused = on_ipod(&w, 2, "something.m4a", IpodTrack { artist: "The Beatles".into(), ..tagged("Something") });
        reused.library_id = Some(other);
        // A song the library has since retitled, or given another artist, is still the one the iPod has.
        let retitled = in_library(&w, "episode_12.mp3");
        let mut stale = on_ipod(
            &w,
            3,
            "episode_12.mp3",
            IpodTrack { artist: "Radiolab".into(), album_artist: "Radiolab".into(), ..tagged("Episode 12, before it was retitled") },
        );
        stale.library_id = Some(retitled);
        let playlists = vec![playlist("a", "Mixed", &[1, 2, 3])];

        let summary = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![gone, reused, stale], playlists));

        assert_eq!((summary.copied, summary.known), (2, 1));
        let titles: Vec<_> = library_tracks(&w).into_iter().map(|track| track.title).collect();
        assert_eq!(titles, ["Episode 12", "Halo", "Something", "Zombie"], "the retitled song is not copied again");
        let mixed = made_playlists(&w).into_iter().find(|playlist| playlist.name == "Mixed").unwrap();
        let in_order = TrackQuery { kind: None, playlist: Some(mixed.id), sort: SortKey::PlaylistOrder, ..TrackQuery::default() };
        let members = w.library.queries().tracks(&in_order).unwrap();
        let member_titles: Vec<_> = members.iter().map(|track| track.title.as_str()).collect();
        assert_eq!(member_titles, ["Halo", "Something", "Episode 12"], "not Zombie, whose number the iPod remembered");
    }

    #[test]
    fn protected_unsure_and_missing_songs_are_counted_and_left_out() {
        let w = world();
        w.library
            .with_writer(|writer| {
                let twin = |id, album: &str| Track {
                    title: "Same Song".into(),
                    album: album.into(),
                    ..crate::test_support::track(id, "MP3", unsub_library::MediaKind::Music)
                };
                writer.import_tracks(&[twin(1, "One Album"), twin(2, "Another Album")])
            })
            .unwrap();
        let fine = on_ipod(&w, 1, "halo.mp3", tagged("Halo"));
        let mut sold =
            on_ipod(&w, 2, "bohemian_rhapsody.m4a", IpodTrack { file_type: "Protected AAC audio file".into(), ..tagged("Bought") });
        let renamed = sold.track.path.with_extension("m4p");
        fs::rename(&sold.track.path, &renamed).unwrap();
        sold.track.path = renamed;
        let undecided = on_ipod(
            &w,
            3,
            "something.m4a",
            IpodTrack { title: "Same Song".into(), artist: "Artist".into(), duration_ms: 180_000, ..IpodTrack::default() },
        );
        let mut gone = on_ipod(&w, 4, "zombie.ogg", tagged("Gone"));
        gone.bytes = None;
        let video = RescueSong {
            track: IpodTrack { id: 5, media_kind: IpodMediaKind::Video, ..tagged("Trailer") },
            library_id: None,
            bytes: Some(10),
        };

        let summary = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![fine, sold, undecided, gone, video], vec![]));

        assert_eq!(
            summary,
            Summary { copied: 1, protected: 1, unsure: 1, failed: 1, ..Summary::default() },
            "a video is not a song and is not counted"
        );
        assert_eq!(files_in_media(&w).len(), 1);
    }

    #[test]
    fn copying_again_adds_no_songs_plays_or_playlists() {
        let w = world();
        let songs: Vec<RescueSong> =
            (1..=3).map(|id| on_ipod(&w, id, "halo.mp3", IpodTrack { play_count: 4 + id, ..tagged(&format!("Song {id}")) })).collect();
        let playlists = vec![playlist("4097", "Road Trip", &[3, 1]), playlist("otg1", "On-The-Go 1", &[2])];
        let rescue = rescue(songs, playlists);
        let ipod = ipod(&w, "Riley's iPod");

        let first = copy_all(&w, &ipod, &rescue);
        let tracks = library_tracks(&w);
        let playlists_made = made_playlists(&w);
        let files = contents(&w.media);
        let recorded = fs::read(&w.mapping).unwrap();
        let items = |name: &str| {
            let library = w.library.queries();
            let id = library.playlists().unwrap().into_iter().find(|playlist| playlist.name == name).unwrap().id;
            library.tracks(&TrackQuery { kind: None, playlist: Some(id), sort: SortKey::PlaylistOrder, ..TrackQuery::default() }).unwrap()
        };

        assert_eq!((first.copied, first.playlists_created, first.playlists_updated), (3, 2, 0));
        let names: Vec<_> = playlists_made.iter().map(|playlist| (playlist.name.as_str(), playlist.kind)).collect();
        assert_eq!(
            names,
            [("From Riley's iPod", PlaylistKind::Folder), ("Road Trip", PlaylistKind::Regular), ("On-The-Go 1", PlaylistKind::Regular)]
        );
        assert_eq!(items("Road Trip").iter().map(|track| track.title.as_str()).collect::<Vec<_>>(), ["Song 3", "Song 1"]);
        assert_eq!(items("On-The-Go 1").iter().map(|track| track.title.as_str()).collect::<Vec<_>>(), ["Song 2"]);

        for _ in 0..2 {
            let again = copy_all(&w, &ipod, &rescue);
            assert_eq!((again.copied, again.known, again.playlists_created, again.playlists_updated), (0, 3, 0, 2));
            assert_eq!(library_tracks(&w), tracks);
            assert_eq!(made_playlists(&w), playlists_made);
            assert_eq!(contents(&w.media), files);
            assert_eq!(fs::read(&w.mapping).unwrap(), recorded);
        }

        // A play made here after the copy is on top of the iPod's count, and copying once more leaves it there.
        let song_1 = by_title(&w, "Song 1");
        w.library.with_writer(|writer| writer.record_play(song_1.id, 1_800_000_000)).unwrap();
        copy_all(&w, &ipod, &rescue);
        copy_all(&w, &ipod, &rescue);
        assert_eq!(by_title(&w, "Song 1").play_count, song_1.play_count + 1);
        assert_eq!(by_title(&w, "Song 2").play_count, 6);
    }

    #[test]
    fn a_renamed_ipod_and_a_changed_playlist_update_what_was_made() {
        let w = world();
        let songs: Vec<RescueSong> = (1..=3).map(|id| on_ipod(&w, id, "halo.mp3", tagged(&format!("Song {id}")))).collect();
        copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue(songs.clone(), vec![playlist("4097", "Road Trip", &[1, 2])]));
        let before = made_playlists(&w);

        let renamed = rescue(songs, vec![playlist("4097", "Road Trip 2", &[3])]);
        let summary = copy_all(&w, &ipod(&w, "Riley's Old iPod"), &renamed);

        assert_eq!((summary.playlists_created, summary.playlists_updated), (0, 1));
        let after = made_playlists(&w);
        assert_eq!(
            after.iter().map(|playlist| playlist.id).collect::<Vec<_>>(),
            before.iter().map(|playlist| playlist.id).collect::<Vec<_>>()
        );
        assert_eq!(after.iter().map(|playlist| playlist.name.as_str()).collect::<Vec<_>>(), ["From Riley's Old iPod", "Road Trip 2"]);
        assert_eq!(after[1].track_count, 1);
    }

    #[test]
    fn stopping_leaves_whole_songs_and_playlists_for_later_and_copying_again_carries_on() {
        let w = world();
        let songs: Vec<RescueSong> = (1..=25).map(|id| on_ipod(&w, id, "halo.mp3", tagged(&format!("Song {id:02}")))).collect();
        let rescue = rescue(songs, vec![playlist("4097", "Everything", &(1..=25).collect::<Vec<_>>())]);
        let ipod = ipod(&w, "Riley's iPod");
        let before = contents(&w.ipod_dir);
        let job = Job { ipod: &ipod, rescue: &rescue, scope: &RescueScope::All, options: EVERYTHING, mapping_path: &w.mapping };
        let cancel = AtomicBool::new(false);

        let stopped = copy(&w.library, &job, &cancel, &mut |done, total, _| {
            assert_eq!(total, 25);
            if done == 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();

        assert!(stopped.stopped);
        assert_eq!(stopped.copied, COPY_CHUNK);
        assert_eq!(library_tracks(&w).len(), COPY_CHUNK);
        assert_eq!(files_in_media(&w).len(), COPY_CHUNK, "{:?}", files_in_media(&w));
        assert!(
            files_in_media(&w).iter().all(|path| !path.file_name().unwrap().to_string_lossy().starts_with('.')),
            "no half-made copy is left"
        );
        assert!(made_playlists(&w).is_empty());
        assert_eq!(contents(&w.ipod_dir), before);

        let finished = copy_all(&w, &ipod, &rescue);

        assert!(!finished.stopped);
        assert_eq!((finished.copied, finished.known), (15, COPY_CHUNK));
        assert_eq!(library_tracks(&w).len(), 25);
        assert_eq!(files_in_media(&w).len(), 25);
        assert_eq!(made_playlists(&w).len(), 2);
        assert_eq!(contents(&w.ipod_dir), before);
    }

    #[test]
    fn a_copy_that_will_not_fit_is_refused_before_anything_is_copied() {
        let w = world();
        let fits = on_ipod(&w, 1, "halo.mp3", tagged("Halo"));
        let mut huge = on_ipod(&w, 2, "something.m4a", tagged("Something"));
        huge.bytes = Some(u64::MAX / 2);

        let error = try_copy(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![fits, huge], vec![]), &RescueScope::All, EVERYTHING).unwrap_err();

        assert!(error.to_string().starts_with("there isn't room in "), "{error}");
        assert!(error.to_string().contains(" for these songs: they need "), "{error}");
        assert!(library_tracks(&w).is_empty());
        assert!(files_in_media(&w).is_empty());
    }

    #[test]
    fn the_scope_limits_which_songs_and_which_playlists() {
        let w = world();
        let songs: Vec<RescueSong> = (1..=4).map(|id| on_ipod(&w, id, "halo.mp3", tagged(&format!("Song {id}")))).collect();
        let rescue = rescue(songs, vec![playlist("a", "First", &[1, 2]), playlist("b", "Second", &[3])]);
        let ipod = ipod(&w, "Riley's iPod");

        let some = try_copy(&w, &ipod, &rescue, &RescueScope::Songs { ids: vec![4] }, EVERYTHING).unwrap();
        assert_eq!((some.copied, some.playlists_created), (1, 0));
        assert_eq!(library_tracks(&w).iter().map(|track| track.title.as_str()).collect::<Vec<_>>(), ["Song 4"]);

        let chosen = try_copy(&w, &ipod, &rescue, &RescueScope::Playlists { keys: vec!["a".into()] }, EVERYTHING).unwrap();
        assert_eq!((chosen.copied, chosen.known, chosen.playlists_created), (2, 0, 1));
        let titles: Vec<_> = library_tracks(&w).into_iter().map(|track| track.title).collect();
        assert_eq!(titles, ["Song 1", "Song 2", "Song 4"]);
        let playlists = made_playlists(&w);
        assert_eq!(playlists.iter().map(|playlist| playlist.name.as_str()).collect::<Vec<_>>(), ["From Riley's iPod", "First"]);
    }

    #[test]
    fn without_playlists_or_counts_asked_for_neither_is_brought() {
        let w = world();
        let song = on_ipod(&w, 1, "halo.mp3", IpodTrack { play_count: 9, rating: 80, ..tagged("Halo") });
        let options = RescueOptions { skip_known: true, play_counts: false, playlists: false, restore_offline: false };

        try_copy(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![song], vec![playlist("a", "First", &[1])]), &RescueScope::All, options)
            .unwrap();

        let halo = by_title(&w, "Halo");
        assert_eq!((halo.play_count, halo.rating), (0, 0));
        assert!(made_playlists(&w).is_empty());
    }

    #[test]
    fn an_ipod_without_a_database_is_described_by_its_files_and_copying_again_adds_nothing() {
        let w = world();
        let bare = IpodTrack::default();
        let songs = vec![on_ipod(&w, 1, "halo.mp3", bare.clone()), on_ipod(&w, 2, "something.m4a", bare)];
        let rescue = Rescue { songs, playlists: Vec::new(), from_files: true };
        let ipod = ipod(&w, "SHUFFLE");

        let first = copy_all(&w, &ipod, &rescue);
        let again = copy_all(&w, &ipod, &rescue);

        assert_eq!((first.copied, first.known), (2, 0));
        assert_eq!((again.copied, again.known), (0, 2));
        let titles: Vec<_> = library_tracks(&w).into_iter().map(|track| track.title).collect();
        assert_eq!(titles, ["Halo", "Something"]);
        assert_eq!(files_in_media(&w).len(), 2);
    }

    /// A blank classic, as the sync tests of the iPod crate make one.
    fn blank_ipod(world: &World) -> Ipod {
        let mount = &world.ipod_dir;
        for dir in ["iTunes", "Device", "Music/F00"] {
            fs::create_dir_all(mount.join("iPod_Control").join(dir)).unwrap();
        }
        let sysinfo = "BoardHwName: Hades\nModelNumStr: xC293\nFirewireGuid: 0x000A27001A2B3C4D\n";
        fs::write(mount.join("iPod_Control/Device/SysInfo"), sysinfo).unwrap();
        unsub_ipod::open(mount).unwrap()
    }

    fn sync_track(library_id: i64, source: &Path, title: &str) -> SyncTrack {
        SyncTrack {
            source_path: source.to_path_buf(),
            title: title.into(),
            artist: "Beyoncé".into(),
            album_artist: "Beyoncé".into(),
            album: "I Am".into(),
            genre: String::new(),
            composer: String::new(),
            year: None,
            track_number: Some(1),
            track_count: None,
            disc_number: None,
            disc_count: None,
            duration_ms: 500,
            bitrate_kbps: Some(32),
            sample_rate: Some(16_000),
            rating: 40,
            play_count: 3,
            last_played: None,
            date_added: 1_500_000_000,
            media_kind: IpodMediaKind::Music,
            compilation: false,
            sound_check: None,
            sort_title: None,
            sort_artist: None,
            sort_album: None,
            sort_album_artist: None,
            artwork_key: None,
            library_id,
        }
    }

    #[test]
    fn a_real_ipod_gives_up_its_songs_and_is_left_byte_for_byte_as_it_was() {
        let w = world();
        let ipod = blank_ipod(&w);
        let ours = in_library(&w, "halo.mp3");
        // Two songs iTunes put there: the app's record of what it synced has only the first.
        let tracks = [
            sync_track(ours, &fixture("halo.mp3"), "Halo"),
            sync_track(900, &fixture("something.m4a"), "Something"),
            sync_track(901, &fixture("episode_12.mp3"), "Episode 12"),
        ];
        let playlists = [SyncPlaylist { name: "Mine".into(), tracks: vec![ours, 900] }];
        let current = unsub_ipod::read_database(&ipod).unwrap();
        let plan = unsub_ipod::plan_sync(&ipod, &current, &tracks, &playlists).unwrap();
        let mut prepare = |track: &SyncTrack| Ok(Prepared::from(track.source_path.clone()));
        unsub_ipod::apply_sync(&ipod, &plan, &tracks, &playlists, &mut prepare, &mut |_| None, &mut |_| {}).unwrap();
        let mapping_path = w.ipod_dir.join("iPod_Control/iTunes/Unsub.json");
        let mut mapping: serde_json::Value = serde_json::from_slice(&fs::read(&mapping_path).unwrap()).unwrap();
        mapping["tracks"].as_array_mut().unwrap().retain(|track| track["libraryId"] == ours);
        fs::write(&mapping_path, serde_json::to_vec(&mapping).unwrap()).unwrap();
        let before = contents(&w.ipod_dir);
        let rescue = unsub_ipod::read_rescue(&ipod).unwrap();

        let first = copy_all(&w, &ipod, &rescue);
        let again = copy_all(&w, &ipod, &rescue);

        assert_eq!(rescue.songs.iter().filter(|song| song.library_id.is_some()).count(), 1);
        assert!(rescue.playlists.is_empty(), "the app's own playlist mirrors the library");
        assert_eq!((first.copied, first.known, first.protected, first.failed), (2, 1, 0, 0));
        assert_eq!((again.copied, again.known), (0, 3));
        let titles: Vec<_> = library_tracks(&w).into_iter().map(|track| track.title).collect();
        assert_eq!(titles, ["Episode 12", "Halo", "Something"]);
        assert_eq!(by_title(&w, "Something").play_count, 3);
        assert_eq!(files_in_media(&w).len(), 2);
        assert_eq!(contents(&w.ipod_dir), before);
        assert!(made_playlists(&w).is_empty());
    }

    #[test]
    fn the_notice_says_what_was_copied_and_what_was_not_in_the_apps_tone() {
        let summary = Summary { copied: 4_812, known: 312, protected: 41, ..Summary::default() };
        assert_eq!(
            notice(&summary, "Riley's iPod"),
            "Copied 4,812 songs from Riley's iPod. 312 were already in your library. 41 are copy-protected and were left out."
        );

        let one = Summary { copied: 1, known: 1, protected: 1, unsure: 1, failed: 2, playlists_created: 1, ..Summary::default() };
        assert_eq!(
            notice(&one, "iPod"),
            "Copied 1 song from iPod. 1 was already in your library. 1 is copy-protected and was left out. \
             1 could be more than one of your songs, so it was left out. 2 couldn't be copied. 1 playlist is in “From iPod”."
        );

        let stopped = Summary { copied: 20, unsure: 3, stopped: true, playlists_updated: 2, ..Summary::default() };
        assert_eq!(
            notice(&stopped, "iPod"),
            "Copy stopped. 20 songs copied from iPod. 3 could be more than one of your songs, so they were left out. \
             2 playlists are in “From iPod”."
        );
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000_000), "1,000,000");
    }

    #[test]
    fn a_library_row_is_the_synced_song_by_both_names_a_little_off_or_one_name_to_the_millisecond() {
        let row = Track {
            title: "Halo".into(),
            artist: "Beyoncé".into(),
            duration_ms: 200_000,
            ..crate::test_support::track(1, "MP3", unsub_library::MediaKind::Music)
        };
        let song = |title: &str, artist: &str, duration_ms: u64| IpodTrack {
            title: title.into(),
            artist: artist.into(),
            duration_ms,
            ..IpodTrack::default()
        };

        // Both names: the matcher's window.
        assert!(agrees(&row, &song("Halo", "Beyoncé", 200_000)));
        assert!(agrees(&row, &song("Halo (Remastered)", "BEYONCÉ feat. Jay-Z", 202_900)));
        assert!(!agrees(&row, &song("Halo", "Beyoncé", 204_000)), "a different length");
        // One name: the same length to within 100 ms, which a synced song has and its neighbour on the album doesn't.
        assert!(agrees(&row, &song("Halo", "Someone Else", 200_090)), "only the title agrees");
        assert!(agrees(&row, &song("Another Name", "Beyoncé", 199_950)), "retitled since: the artist agrees");
        assert!(!agrees(&row, &song("Another Name", "Beyoncé", 201_500)), "the next song on the album, 1.5 s longer");
        assert!(!agrees(&row, &song("Halo", "Someone Else", 202_000)));
        assert!(!agrees(&row, &song("Another Name", "Someone Else", 200_000)), "neither name");
        // No length to go on, as for a placeholder: both names or nothing.
        let placeholder = Track { duration_ms: 0, ..row.clone() };
        assert!(agrees(&placeholder, &song("Halo", "Beyoncé", 200_000)));
        assert!(!agrees(&placeholder, &song("Halo", "Someone Else", 200_000)));
        assert!(!agrees(&placeholder, &song("Another Name", "Beyoncé", 200_000)));
        assert!(!agrees(&row, &song("Halo", "Someone Else", 0)), "an iPod song of unknown length is no more to go on");
        assert!(agrees(&row, &song("Halo", "Beyoncé", 0)));
        let nameless = Track { title: String::new(), artist: String::new(), ..row.clone() };
        assert!(!agrees(&nameless, &song("", "", 200_000)), "no name is not a name that agrees");
    }

    #[test]
    fn a_mapped_id_that_now_belongs_to_the_next_song_on_the_album_is_not_believed() {
        let w = world();
        w.library
            .with_writer(|writer| {
                let neighbour = Track {
                    title: "Next Song".into(),
                    artist: "Beyoncé".into(),
                    album: "I Am".into(),
                    duration_ms: 2_000,
                    ..crate::test_support::track(7, "MP3", unsub_library::MediaKind::Music)
                };
                writer.import_tracks(&[neighbour])
            })
            .unwrap();
        let mut song = on_ipod(&w, 1, "halo.mp3", tagged("Halo"));
        song.library_id = Some(7);

        let summary = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![song], vec![]));

        assert_eq!((summary.copied, summary.known), (1, 0));
        assert_eq!(library_tracks(&w).len(), 2);
    }

    #[test]
    fn a_rescued_songs_record_goes_with_it_so_a_reused_id_is_never_taken_for_it() {
        let w = world();
        let nameless = IpodTrack { dbid: 0x1001, title: String::new(), artist: String::new(), duration_ms: 500, ..IpodTrack::default() };
        let song = on_ipod(&w, 1, "halo.mp3", nameless);
        let rescue = rescue(vec![song], vec![]);
        let first = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue);
        let id = library_tracks(&w)[0].id;
        assert_eq!(first.copied, 1);
        assert_eq!(copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue).known, 1);

        // The user deletes the song, and the next one added is given its number.
        w.library.with_writer(|writer| writer.remove_tracks(&[id])).unwrap();
        let other = in_library(&w, "zombie.ogg");
        assert_eq!(other, id, "the id is used again");
        let again = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue);

        // The new row is not the rescued song just because it has the number, and the song, wanted back, comes again.
        assert_eq!((again.copied, again.known), (1, 0));
        assert_eq!(library_tracks(&w).len(), 2);
        assert_eq!(copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue).known, 1);
    }

    #[test]
    fn a_song_on_a_disk_that_is_not_connected_is_left_alone_not_restored() {
        let w = world();
        w.library
            .with_writer(|writer| {
                let on_the_drive = Track {
                    title: "Halo".into(),
                    artist: "Beyoncé".into(),
                    path: "/Volumes/Surely Not Mounted Drive/Beyoncé/Halo.mp3".into(),
                    duration_ms: 500,
                    missing: true,
                    play_count: 3,
                    ..crate::test_support::track(1, "MP3", unsub_library::MediaKind::Music)
                };
                writer.import_tracks(&[on_the_drive])
            })
            .unwrap();
        let song = on_ipod(&w, 1, "halo.mp3", IpodTrack { play_count: 7, ..tagged("Halo") });
        let rescue = rescue(vec![song], vec![]);

        let summary = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue);

        assert_eq!((summary.copied, summary.restored, summary.known, summary.offline), (0, 0, 0, 1));
        assert!(files_in_media(&w).is_empty());
        let row = by_title(&w, "Halo");
        assert!(row.missing && row.path.starts_with("/Volumes/Surely Not Mounted Drive/"));
        assert_eq!(row.play_count, 7, "its counts are still brought");
        assert_eq!(notice(&summary, "iPod"), "Nothing new to copy from iPod. 1 is on a disk that isn't connected and was left alone.");
    }

    #[test]
    fn songs_on_a_disk_that_is_gone_for_good_are_restored_when_asked_keeping_their_counts_and_playlists() {
        let w = world();
        let id = w
            .library
            .with_writer(|writer| {
                let on_the_drive = Track {
                    title: "Halo".into(),
                    artist: "Beyoncé".into(),
                    path: "/Volumes/Backup Drive/Beyoncé/Halo.mp3".into(),
                    duration_ms: 500,
                    missing: true,
                    play_count: 3,
                    ..crate::test_support::track(1, "MP3", unsub_library::MediaKind::Music)
                };
                writer.import_tracks(&[on_the_drive])?;
                let playlist = writer.create_playlist("Mine", PlaylistKind::Regular, None)?;
                writer.add_to_playlist(playlist.id, &[1], None)?;
                Ok(1)
            })
            .unwrap();
        let song = on_ipod(&w, 1, "halo.mp3", IpodTrack { play_count: 7, ..tagged("Halo") });
        let rescue = rescue(vec![song], vec![]);
        let ipod = ipod(&w, "Riley's iPod");
        let job = |options| Job { ipod: &ipod, rescue: &rescue, scope: &RescueScope::All, options, mapping_path: &w.mapping };
        let asked = RescueOptions { restore_offline: true, ..EVERYTHING };

        // Either way the sheet is told which disk it is, so that it can offer the choice.
        let left = make_plan(&w.library, &job(EVERYTHING), &|| false).unwrap().unwrap();
        let counted = make_plan(&w.library, &job(asked), &|| false).unwrap().unwrap();
        assert_eq!((left.offline, left.restored(), counted.offline, counted.restored()), (1, 0, 0, 1));
        let both = (left.offline_volumes, counted.offline_volumes);
        assert_eq!(both, (BTreeSet::from(["Backup Drive".to_owned()]), BTreeSet::from(["Backup Drive".to_owned()])));

        let summary = try_copy(&w, &ipod, &rescue, &RescueScope::All, asked).unwrap();

        assert_eq!((summary.restored, summary.copied, summary.offline, summary.known), (1, 0, 0, 0));
        let row = by_title(&w, "Halo");
        assert_eq!((row.id, row.missing, row.play_count), (id, false, 7));
        assert!(Path::new(&row.path).starts_with(fs::canonicalize(&w.media).unwrap()) && Path::new(&row.path).is_file());
        assert_eq!(library_tracks(&w).len(), 1);
        let mine = w.library.queries().playlists().unwrap().into_iter().find(|playlist| playlist.name == "Mine").unwrap();
        assert_eq!(mine.track_count, 1);
    }

    #[test]
    fn only_a_file_gone_from_a_disk_that_is_there_counts_as_lost() {
        let unmounted = "/Volumes/Surely Not Mounted Drive/a/b.mp3";
        assert_eq!(unplugged_volume(Path::new(unmounted)).as_deref(), Some("Surely Not Mounted Drive"));
        assert_eq!(unplugged_volume(Path::new("/Volumes")), None);
        assert_eq!(
            unplugged_volume(Path::new("/definitely/not/there/b.mp3")),
            None,
            "a missing folder on the disk the app runs from is lost"
        );
        assert_eq!(unplugged_volume(Path::new("relative/b.mp3")), None);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(unplugged_volume(&dir.path().join("gone.mp3")), None);
    }

    #[test]
    fn a_disk_that_came_back_before_the_song_was_restored_is_left_as_it_is() {
        let w = world();
        let id = lost_from_library(&w, "halo.mp3");
        let song = on_ipod(&w, 1, "halo.mp3", tagged("Halo"));
        let before = by_title(&w, "Halo");
        // The plan is made with the song missing; then the drive is plugged in, and the song is back where it was.
        let rescue = rescue(vec![song], vec![]);
        let ipod = ipod(&w, "Riley's iPod");
        let job = Job { ipod: &ipod, rescue: &rescue, scope: &RescueScope::All, options: EVERYTHING, mapping_path: &w.mapping };
        let summary = copy(&w.library, &job, &AtomicBool::new(false), &mut |_, _, _| {
            fs::copy(fixture("halo.mp3"), &before.path).unwrap();
        })
        .unwrap();

        assert_eq!((summary.copied, summary.restored, summary.known, summary.failed), (0, 0, 1, 0));
        assert_eq!(by_title(&w, "Halo").path, before.path);
        assert_eq!(by_title(&w, "Halo").id, id);
        assert!(files_in_media(&w).is_empty());
    }

    #[test]
    fn a_restored_song_takes_the_audio_details_of_the_ipods_file() {
        let w = world();
        let id = lost_from_library(&w, "come_together.flac");
        w.library
            .with_writer(|writer| {
                writer.set_loudness(id, &unsub_library::Loudness { lufs: -9.0, true_peak_dbtp: -0.5, state: Some(vec![1]) })
            })
            .unwrap();
        let song = on_ipod(
            &w,
            1,
            "bohemian_rhapsody.m4a",
            IpodTrack { title: "Come Together".into(), artist: "The Beatles".into(), duration_ms: 500, ..IpodTrack::default() },
        );
        assert_eq!(by_title(&w, "Come Together").codec, "FLAC");

        let summary = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![song], vec![]));

        assert_eq!(summary.restored, 1);
        let row = by_title(&w, "Come Together");
        assert_eq!((row.id, row.codec.as_str()), (id, "AAC"));
        assert!(row.loudness.is_none());
    }

    /// A song the library lists whose file is gone, as when the drive it was on died.
    fn lost_from_library(world: &World, fixture_name: &str) -> i64 {
        let id = in_library(world, fixture_name);
        let path = world._root.path().join(format!("mine-{fixture_name}"));
        fs::remove_file(path).unwrap();
        world.library.with_writer(|writer| writer.check_missing()).unwrap();
        id
    }

    #[test]
    fn a_song_the_library_lost_the_file_of_is_restored_in_place_and_not_added() {
        let w = world();
        let found = lost_from_library(&w, "halo.mp3");
        let synced = lost_from_library(&w, "something.m4a");
        let playlist_id = w
            .library
            .with_writer(|writer| {
                let playlist = writer.create_playlist("Mine", PlaylistKind::Regular, None)?;
                writer.add_to_playlist(playlist.id, &[found, synced], None)?;
                Ok(playlist.id)
            })
            .unwrap();
        // One the matcher finds, and one Unsub synced (whose plays the sync, not this, brings back).
        let first = on_ipod(&w, 1, "halo.mp3", IpodTrack { play_count: 7, rating: 60, ..tagged("Halo") });
        let mut second = on_ipod(&w, 2, "something.m4a", IpodTrack { play_count: 9, artist: "The Beatles".into(), ..tagged("Something") });
        second.library_id = Some(synced);
        let before = contents(&w.ipod_dir);

        let summary = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![first, second], vec![]));

        assert_eq!((summary.copied, summary.restored, summary.known, summary.failed), (0, 2, 0, 0));
        assert_eq!(library_tracks(&w).len(), 2, "no new rows");
        let media = fs::canonicalize(&w.media).unwrap();
        for (title, id, plays, rating) in [("Halo", found, 7, 60), ("Something", synced, 0, 0)] {
            let row = by_title(&w, title);
            assert_eq!(row.id, id);
            assert!(!row.missing && Path::new(&row.path).starts_with(&media) && Path::new(&row.path).is_file(), "{row:?}");
            assert_eq!((row.play_count, row.rating), (plays, rating), "{title}");
        }
        assert_eq!(files_in_media(&w).len(), 2);
        let kept = w.library.queries().tracks(&TrackQuery { kind: None, playlist: Some(playlist_id), ..TrackQuery::default() }).unwrap();
        assert_eq!(kept.len(), 2, "the playlist keeps both");
        assert_eq!(contents(&w.ipod_dir), before);

        // Once restored it is just a song the library has.
        let again = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![on_ipod(&w, 1, "halo.mp3", tagged("Halo"))], vec![]));
        assert_eq!((again.copied, again.restored, again.known), (0, 0, 1));
    }

    #[test]
    fn a_song_that_cannot_be_matched_is_copied_once_however_often_it_is_copied() {
        let w = world();
        // No title and no artist to key on, and a title like these is on a hundred albums.
        let nameless = |id: u32| IpodTrack {
            dbid: 0x1000 + u64::from(id),
            title: String::new(),
            artist: String::new(),
            duration_ms: 500,
            ..IpodTrack::default()
        };
        let songs: Vec<RescueSong> = (1..=3).map(|id| on_ipod(&w, id, "halo.mp3", nameless(id))).collect();
        let rescue = rescue(songs, vec![]);
        let ipod = ipod(&w, "Riley's iPod");

        let first = copy_all(&w, &ipod, &rescue);
        assert_eq!((first.copied, first.known), (3, 0));
        // The user tidies the copies, which changes what the matcher would have to go on.
        let retitled = library_tracks(&w).into_iter().next().unwrap().id;
        w.library
            .with_writer(|writer| {
                writer.edit_tracks(&[retitled], &unsub_library::TrackEdits { title: Some("Retitled".into()), ..Default::default() })
            })
            .unwrap();
        let again = copy_all(&w, &ipod, &rescue);

        assert_eq!((again.copied, again.known), (0, 3));
        assert_eq!(library_tracks(&w).len(), 3);
        assert_eq!(files_in_media(&w).len(), 3);
    }

    #[test]
    fn stopping_and_copying_again_carries_on_without_duplicating_songs_it_cannot_match() {
        let w = world();
        let nameless = |id: u32| IpodTrack {
            dbid: 0x1000 + u64::from(id),
            title: String::new(),
            artist: String::new(),
            duration_ms: 500,
            ..IpodTrack::default()
        };
        let songs: Vec<RescueSong> = (1..=25).map(|id| on_ipod(&w, id, "halo.mp3", nameless(id))).collect();
        let rescue = rescue(songs, vec![]);
        let ipod = ipod(&w, "Riley's iPod");
        let job = Job { ipod: &ipod, rescue: &rescue, scope: &RescueScope::All, options: EVERYTHING, mapping_path: &w.mapping };
        let cancel = AtomicBool::new(false);

        let stopped = copy(&w.library, &job, &cancel, &mut |_, _, _| cancel.store(true, Ordering::Relaxed)).unwrap();
        let finished = copy_all(&w, &ipod, &rescue);

        assert_eq!((stopped.stopped, stopped.copied), (true, COPY_CHUNK));
        assert_eq!((finished.copied, finished.known), (25 - COPY_CHUNK, COPY_CHUNK));
        assert_eq!(library_tracks(&w).len(), 25);
        assert_eq!(files_in_media(&w).len(), 25);
    }

    #[test]
    fn an_untagged_ipod_without_a_database_is_copied_once() {
        let w = world();
        let bare = IpodTrack::default();
        let rescue = Rescue { songs: vec![on_ipod(&w, 1, "untagged.wav", bare)], playlists: Vec::new(), from_files: true };
        let ipod = ipod(&w, "SHUFFLE");

        let first = copy_all(&w, &ipod, &rescue);
        let again = copy_all(&w, &ipod, &rescue);

        assert_eq!((first.copied, again.copied, again.known), (1, 0, 1));
        assert_eq!(library_tracks(&w).len(), 1);
    }

    #[test]
    fn without_skipping_known_songs_every_song_is_copied_even_one_that_is_unsure() {
        let w = world();
        w.library
            .with_writer(|writer| {
                let twin = |id, album: &str| Track {
                    title: "Same Song".into(),
                    album: album.into(),
                    ..crate::test_support::track(id, "MP3", unsub_library::MediaKind::Music)
                };
                writer.import_tracks(&[twin(1, "One Album"), twin(2, "Another Album")])
            })
            .unwrap();
        let song = on_ipod(
            &w,
            1,
            "halo.mp3",
            IpodTrack { title: "Same Song".into(), artist: "Artist".into(), duration_ms: 180_000, ..IpodTrack::default() },
        );
        let rescue = rescue(vec![song], vec![]);
        let all = RescueOptions { skip_known: false, ..EVERYTHING };

        let summary = try_copy(&w, &ipod(&w, "Riley's iPod"), &rescue, &RescueScope::All, all).unwrap();
        let skipping = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue);

        assert_eq!((summary.copied, summary.unsure), (1, 0));
        // Skipping, the copy just made is a third song that fits, on yet another album, and still nothing says which.
        assert_eq!((skipping.copied, skipping.known), (0, 1), "the ledger knows the one copied before");
    }

    #[test]
    fn a_version_a_few_seconds_off_is_copied_and_counted_as_another_version() {
        let w = world();
        w.library
            .with_writer(|writer| {
                writer.import_tracks(&[Track {
                    title: "Halo".into(),
                    artist: "Beyoncé".into(),
                    album: "I Am".into(),
                    duration_ms: 180_000,
                    ..crate::test_support::track(1, "MP3", unsub_library::MediaKind::Music)
                }])
            })
            .unwrap();
        let song = on_ipod(&w, 1, "halo.mp3", IpodTrack { duration_ms: 185_000, ..tagged("Halo") });

        let summary = copy_all(&w, &ipod(&w, "Riley's iPod"), &rescue(vec![song], vec![]));

        assert_eq!((summary.copied, summary.similar, summary.known), (1, 1, 0));
        assert_eq!(library_tracks(&w).len(), 2);
    }

    #[test]
    fn a_protected_file_with_an_innocent_name_is_found_just_before_it_is_copied() {
        let w = world();
        let fine = on_ipod(&w, 1, "halo.mp3", tagged("Halo"));
        let disguised = on_ipod(&w, 2, "something.m4a", tagged("Something"));
        fs::write(&disguised.track.path, mp4_with_entry(b"drms")).unwrap();
        let rescue = rescue(vec![fine, disguised], vec![]);
        let ipod = ipod(&w, "Riley's iPod");
        let job = Job { ipod: &ipod, rescue: &rescue, scope: &RescueScope::All, options: EVERYTHING, mapping_path: &w.mapping };

        let plan = make_plan(&w.library, &job, &|| false).unwrap().unwrap();
        let summary = copy_all(&w, &ipod, &rescue);

        assert_eq!((plan.protected, plan.todo.len()), (0, 2), "counting does not open the files");
        assert_eq!((summary.copied, summary.protected, summary.failed), (1, 1, 0));
        assert_eq!(library_tracks(&w).len(), 1);
    }

    #[test]
    fn a_copy_stopped_while_it_is_still_planning_copies_nothing() {
        let w = world();
        let song = on_ipod(&w, 1, "halo.mp3", tagged("Halo"));
        let ipod = ipod(&w, "Riley's iPod");
        let rescue = rescue(vec![song], vec![]);
        let job = Job { ipod: &ipod, rescue: &rescue, scope: &RescueScope::All, options: EVERYTHING, mapping_path: &w.mapping };

        let summary = copy(&w.library, &job, &AtomicBool::new(true), &mut |_, _, _| {}).unwrap();

        assert_eq!(summary, Summary { stopped: true, ..Summary::default() });
        assert!(library_tracks(&w).is_empty());
    }

    #[test]
    fn an_ipod_pulled_out_part_way_ends_the_copy_without_blaming_the_songs() {
        let w = world();
        let songs: Vec<RescueSong> = (1..=25).map(|id| on_ipod(&w, id, "halo.mp3", tagged(&format!("Song {id:02}")))).collect();
        let rescue = rescue(songs, vec![playlist("a", "All of it", &(1..=25).collect::<Vec<_>>())]);
        let ipod = ipod(&w, "Riley's iPod");
        let job = Job { ipod: &ipod, rescue: &rescue, scope: &RescueScope::All, options: EVERYTHING, mapping_path: &w.mapping };

        let summary = copy(&w.library, &job, &AtomicBool::new(false), &mut |done, _, _| {
            if done == COPY_CHUNK {
                fs::remove_dir_all(&w.ipod_dir).unwrap();
            }
        })
        .unwrap();

        assert_eq!((summary.disconnected, summary.copied, summary.failed), (true, COPY_CHUNK, 0));
        assert!(made_playlists(&w).is_empty());
        assert_eq!(notice(&summary, "Riley's iPod"), "The iPod was disconnected. 10 songs were copied.");
    }

    #[test]
    fn a_chunk_is_ten_songs_or_sixty_four_megabytes_but_never_nothing() {
        let sized = |bytes: &[u64]| -> Vec<RescueSong> {
            bytes.iter().map(|&size| RescueSong { track: IpodTrack::default(), library_id: None, bytes: Some(size) }).collect()
        };
        let todo_of = |songs: &'static [RescueSong]| -> Vec<Todo<'static>> {
            songs.iter().map(|song| Todo { song, restores: None, stats: true }).collect()
        };
        let mb = 1 << 20;
        let small: &'static [RescueSong] = Box::leak(sized(&[mb; 40]).into_boxed_slice());
        let books: &'static [RescueSong] = Box::leak(sized(&[200 * mb, 200 * mb, 10 * mb]).into_boxed_slice());
        let mixed: &'static [RescueSong] = Box::leak(sized(&[30 * mb, 30 * mb, 30 * mb, mb]).into_boxed_slice());

        assert_eq!(next_chunk(&todo_of(small)).len(), COPY_CHUNK);
        assert_eq!(next_chunk(&todo_of(books)).len(), 1, "a book is a chunk of its own");
        assert_eq!(next_chunk(&todo_of(mixed)).len(), 2, "two fit in 64 MB, a third would not");
        assert!(next_chunk(&[]).is_empty());
    }

    #[test]
    fn the_notice_also_says_what_was_restored_what_looked_like_other_versions_and_when_nothing_was_new() {
        let restored = Summary { copied: 2, restored: 1, similar: 1, known: 4, ..Summary::default() };
        assert_eq!(
            notice(&restored, "iPod"),
            "Copied 2 songs from iPod. Restored 1 song whose file was missing from your library. \
             1 looks like another version of songs you have and was copied too. 4 were already in your library."
        );

        let only_restored = Summary { restored: 3, similar: 2, ..Summary::default() };
        assert_eq!(
            notice(&only_restored, "iPod"),
            "Restored 3 songs whose files were missing from your library. 2 look like other versions of songs you have and were copied too."
        );

        let nothing = Summary { known: 5, ..Summary::default() };
        assert_eq!(notice(&nothing, "iPod"), "Nothing new to copy from iPod. 5 were already in your library.");
        assert_eq!(notice(&Summary::default(), "iPod"), "Nothing new to copy from iPod.");
    }
}
