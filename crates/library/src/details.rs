//! Getting the real details of songs that are only in iCloud. The library knows them by their file
//! names (no genre, no cover, no length) until their files are opened, which downloads them, so
//! this fetches them one at a time, reads each, keeps its cover, and gives the file back.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{OptionalExtension, params};

use crate::cloud::{POLL_INTERVAL, Storage, System, evict_with, wait_for_local};
use crate::error::{Error, Result};
use crate::library::Library;
use crate::organize::flag;

/// What a download leaves free on the disk, at least: the owner's disk may be nearly full, and a
/// file that does not fit would not be the only thing to fail.
const SPARE_BYTES: u64 = 10_000_000_000;
/// A big song needs more than that: the file itself, and room for whatever else is going on.
const SPARE_PER_SONG: u64 = 3;
/// Songs taken from the database at a time.
const BATCH: usize = 100;
/// How many songs are read between two tellings that the library changed; each makes the window
/// load its song list again.
const CHANGED_EVERY: u64 = 50;
/// Giving a file back takes the system a while, so space comes back late. Each wait for it is
/// twice the one before.
const SPACE_WAITS: [Duration; 6] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
];
/// How many polls, at most, the last files are waited for to become placeholders again.
const FINISH_POLLS: u32 = 40;
/// A download that has brought no more of the file in this long is given up on. How much of a file
/// is on disk may not move until it is all there, so a big file is given longer, as if it came at
/// `SLOWEST_DOWNLOAD`.
const DOWNLOAD_STALL: Duration = Duration::from_secs(180);
/// Bytes a second, below which a download is not expected to be going.
const SLOWEST_DOWNLOAD: u64 = 256 * 1024;
/// Stalled downloads in a row after which iCloud is taken not to be answering.
const STALLS_BEFORE_OFFLINE: u8 = 3;
/// Songs in a row whose files are not there after which the folder is taken to be gone.
const MISSING_BEFORE_BLOCK: u8 = 5;
/// Times the system is asked to take a file back before it is left where it is. A file can be
/// briefly busy (Spotlight indexing it), but not for ever.
const EVICTION_TRIES: u8 = 3;
/// Files left where they are, in a row, after which giving back is taken not to work here.
const REFUSALS_BEFORE_BLOCK: u8 = 3;

const WANTED: &str = "cloud_details_wanted";
/// Placeholders whose tags were never read (that is what a `parser_version` of 0 says; a newer tag
/// parser is no reason to download them all again), inside `?1` (empty for anywhere), and not
/// known to be unreadable as they are.
const UNREAD: &str = "in_cloud = 1 AND parser_version = 0 AND missing = 0
    AND substr(path, 1, length(?1)) = ?1
    AND NOT EXISTS (SELECT 1 FROM unreadable_songs u WHERE u.path = tracks.path
        AND u.file_size = tracks.file_size AND u.date_modified = tracks.date_modified)";

/// A song the library knows only by its file name: its file is in the cloud and no one has opened
/// it since.
pub(crate) struct UnreadSong {
    pub id: i64,
    pub title: String,
    pub path: String,
    pub file_size: u64,
}

/// How many songs are waiting to be read, and what their files come to: what reading them downloads.
pub struct UnreadTotals {
    pub songs: u64,
    pub bytes: u64,
}

/// Where a read stands, told before the slow work on each song.
pub struct DetailsProgress {
    pub read: u64,
    pub skipped: u64,
    pub missing: u64,
    /// The songs there were to read when it began.
    pub total: u64,
    /// The song in hand, as the library knows it by now.
    pub title: String,
    /// The song is about to be downloaded, which takes a while, so this report is worth showing
    /// however soon it follows the last. Others come as fast as songs can be passed over.
    pub download: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DetailsSummary {
    pub read: u64,
    /// Songs that could not be fetched or read. Those that could not be fetched are tried again
    /// the next time, not now; those that came down unreadable wait until their file changes.
    pub skipped: u64,
    /// Songs whose files are not there.
    pub missing: u64,
}

/// Why the read cannot go on until the user says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// The disk stayed too full for the next download, after every wait.
    LowSpace { free: u64, needed: u64 },
    /// Several downloads in a row brought in nothing: iCloud is not answering, or the Mac is offline.
    NoResponse,
    /// Several songs in a row have no file: iCloud Drive is off, or the folder has moved.
    FilesMissing,
    /// The system left several downloaded files where they are, and going on would fill the disk.
    NotGivenBack,
}

/// What the read leaves to the app around it: when it may go on, what it must not touch, and how
/// to tell the user.
pub trait DetailsHost {
    fn cancel_flag(&self) -> &AtomicBool;
    /// Called before every song. Returns once the read may go on (the user has not paused it,
    /// playback is not fetching a song of its own), or false if it was stopped meanwhile.
    fn wait_turn(&self) -> bool;
    /// Bytes free on the volume that holds `path`.
    fn free_bytes(&self, path: &Path) -> u64;
    /// The ids of the songs playing or about to, whose files must stay where they are.
    fn in_use(&self) -> Vec<i64>;
    /// Returns once the user wants the read tried again, or false if it was stopped. Waiting a
    /// few more seconds would not help, and going on would only fail song after song.
    fn blocked(&self, reason: Blocked) -> bool;
    fn progress(&self, progress: &DetailsProgress);
    /// Songs have changed, after a batch of them and once more at the end.
    fn changed(&self);
    /// A file is about to be asked of the cloud. The host keeps the path until `given_back`, so
    /// that a file which arrives after the read gave up on it, or is held when the app quits,
    /// is still found and given back by someone.
    fn fetching(&self, path: &Path);
    /// Whether the file was asked of the cloud by a read that never gave it back.
    fn fetched_earlier(&self, path: &Path) -> bool;
    /// The file is a placeholder again.
    fn given_back(&self, path: &Path);
}

struct Pacing<'a> {
    poll: Duration,
    space_waits: &'a [Duration],
    finish_polls: u32,
    changed_every: u64,
    stall: Duration,
}

impl Library {
    /// How many songs are waiting to be read inside `within` (empty for anywhere), and the size
    /// of their files.
    pub fn unread_cloud_totals(&self, within: &Path) -> Result<UnreadTotals> {
        Ok(self.conn.query_row(
            &format!("SELECT COUNT(*), COALESCE(SUM(file_size), 0) FROM tracks WHERE {UNREAD}"),
            [within.to_string_lossy()],
            |row| {
                Ok(UnreadTotals {
                    songs: row.get(0)?,
                    bytes: row.get(1)?,
                })
            },
        )?)
    }

    /// The next songs waiting to be read after `after_id`, in id order. Going by id rather than
    /// asking again for the first few means a song that fails is passed over, not retried at once.
    pub(crate) fn unread_cloud_songs(
        &self,
        within: &Path,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<UnreadSong>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT id, title, path, file_size FROM tracks WHERE {UNREAD} AND id > ?2
             ORDER BY id LIMIT ?3"
        ))?;
        let songs = stmt.query_map(
            params![within.to_string_lossy(), after_id, limit as i64],
            |row| {
                Ok(UnreadSong {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    path: row.get(2)?,
                    file_size: row.get(3)?,
                })
            },
        )?;
        Ok(songs.collect::<rusqlite::Result<_>>()?)
    }

    /// Whether the library folder of the first song waiting to be read (see `read_cloud_details`)
    /// is there. It is true when nothing waits, when there is no root to go by, and when the song
    /// does not live below it, since nothing can then be said against it.
    pub fn unread_cloud_library_there(&self, within: &Path, root: Option<&Path>) -> Result<bool> {
        let Some(root) = root else {
            return Ok(true);
        };
        let first = self.unread_cloud_songs(within, 0, 1)?;
        Ok(first
            .first()
            .and_then(|song| library_folder(root, Path::new(&song.path)))
            .is_none_or(|folder| folder.exists()))
    }

    /// Whether the user has a read going that a restart should carry on.
    pub fn cloud_details_wanted(&self) -> Result<bool> {
        Ok(self.setting(WANTED)?.is_some_and(|value| value == "1"))
    }

    pub fn set_cloud_details_wanted(&mut self, wanted: bool) -> Result<()> {
        self.put_setting(WANTED, flag(wanted))
    }

    /// Reads the details of every song inside `within` (empty for anywhere) that is only in
    /// iCloud, one at a time, until none is left or `host` stops it. `root` is the folder all of
    /// them live under, if there is one to name, and each song's library is the first folder
    /// below it (iCloud Drive's Music folder, say). While a song's library is there, a song with
    /// no file was deleted; when it is not (iCloud Drive is off, the folder was renamed, a disk is
    /// unplugged) the library is without its files, and no song in it is to be blamed. Each file is fetched, read
    /// through `refresh_cloud_status`, its cover kept, and given back, but only if this fetched
    /// it. Nothing is ever written to a music file. A song that cannot be fetched or read is
    /// counted and passed over. Where it got to is in the database, so a later call carries on
    /// from there.
    pub fn read_cloud_details(
        &mut self,
        within: &Path,
        root: Option<&Path>,
        host: &dyn DetailsHost,
    ) -> Result<DetailsSummary> {
        self.read_details_with(
            within,
            root,
            host,
            &System,
            &Pacing {
                poll: POLL_INTERVAL,
                space_waits: &SPACE_WAITS,
                finish_polls: FINISH_POLLS,
                changed_every: CHANGED_EVERY,
                stall: DOWNLOAD_STALL,
            },
        )
    }

    fn read_details_with(
        &mut self,
        within: &Path,
        root: Option<&Path>,
        host: &dyn DetailsHost,
        storage: &dyn Storage,
        pacing: &Pacing,
    ) -> Result<DetailsSummary> {
        let mut reading = Reading {
            library: self,
            within,
            root,
            host,
            storage,
            pacing,
            owed: Vec::new(),
            summary: DetailsSummary::default(),
            since_changed: 0,
            stalls: 0,
            missing_in_a_row: 0,
            refused: 0,
        };
        let read = reading.run();
        // Whatever stopped the read, the files it fetched still go back.
        let returned = reading.give_back();
        // The last songs were handed back after the last batch was announced, and those flagged
        // missing have not been announced at all.
        if reading.summary.read + reading.summary.missing > 0 {
            host.changed();
        }
        read.and(returned)?;
        let summary = reading.summary;
        // Not worth failing a read that is over for.
        let _ = self.tidy_cloud_covers();
        Ok(summary)
    }

    /// Whether the song's tags have been read, by this read or by anything else that opened its
    /// file since the read began (playback, a sync, a rescan).
    fn details_read(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT parser_version > 0 FROM tracks WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    fn mark_missing(&self, id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE tracks SET missing = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Notes that the song's file, as it is now, came down and could not be read, so that it is
    /// not downloaded again until it changes; or that it could be after all.
    fn remember_unreadable(&self, id: i64, unreadable: bool) -> Result<()> {
        if unreadable {
            self.conn.execute(
                "INSERT OR REPLACE INTO unreadable_songs (path, file_size, date_modified)
                 SELECT path, file_size, date_modified FROM tracks WHERE id = ?1",
                [id],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM unreadable_songs WHERE path = (SELECT path FROM tracks WHERE id = ?1)",
                [id],
            )?;
        }
        Ok(())
    }
}

/// The first folder below `root` that the song lives in: the library it belongs to, such as the
/// Music folder. A song directly in `root` has the root for its library. `None` if the song is
/// not below the root.
fn library_folder(root: &Path, song: &Path) -> Option<PathBuf> {
    let mut parts = song.strip_prefix(root).ok()?.components();
    let first = parts.next()?;
    Some(if parts.next().is_some() {
        root.join(first)
    } else {
        root.to_path_buf()
    })
}

/// A file this read fetched, which it owes back to the cloud.
struct Fetched {
    id: i64,
    path: PathBuf,
    /// Asked to be given back, which the system does in its own time.
    released: bool,
    /// Times the system has declined.
    refusals: u8,
}

enum Outcome {
    Read,
    Skipped,
    Missing,
    Stopped,
}

struct Reading<'a> {
    library: &'a mut Library,
    within: &'a Path,
    root: Option<&'a Path>,
    host: &'a dyn DetailsHost,
    storage: &'a dyn Storage,
    pacing: &'a Pacing<'a>,
    owed: Vec<Fetched>,
    summary: DetailsSummary,
    since_changed: u64,
    /// Downloads in a row that stalled.
    stalls: u8,
    /// Songs in a row with no file while their root is gone too.
    missing_in_a_row: u8,
    /// Files in a row the system has left where they are.
    refused: u8,
}

/// How a download ended.
enum Fetch {
    Here,
    Failed,
    /// Nothing came in for too long. The request is still out there, and the file may arrive
    /// later, unclaimed.
    Stalled,
    Stopped,
}

impl Reading<'_> {
    fn run(&mut self) -> Result<()> {
        let total = self.library.unread_cloud_totals(self.within)?.songs;
        let mut after = 0;
        loop {
            let songs = self.library.unread_cloud_songs(self.within, after, BATCH)?;
            let Some(last) = songs.last() else {
                return Ok(());
            };
            after = last.id;
            for song in songs {
                if !self.host.wait_turn() {
                    return Ok(());
                }
                match self.read_song(&song, total)? {
                    Outcome::Read => {
                        self.summary.read += 1;
                        self.since_changed += 1;
                    }
                    Outcome::Skipped => self.summary.skipped += 1,
                    Outcome::Missing => self.summary.missing += 1,
                    Outcome::Stopped => return Ok(()),
                }
                self.settle()?;
                if self.refused >= REFUSALS_BEFORE_BLOCK {
                    self.refused = 0;
                    if !self.host.blocked(Blocked::NotGivenBack) {
                        return Ok(());
                    }
                }
                if self.since_changed >= self.pacing.changed_every {
                    self.host.changed();
                    self.since_changed = 0;
                }
            }
        }
    }

    fn read_song(&mut self, song: &UnreadSong, total: u64) -> Result<Outcome> {
        let path = Path::new(&song.path);
        // A stat, which never downloads.
        if fs::metadata(path).is_err_and(|error| error.kind() == ErrorKind::NotFound) {
            return self.lost(song);
        }
        self.missing_in_a_row = 0;
        // The library may be behind: a song downloaded since is read as it is, and is not ours
        // to give back, unless an earlier read asked for it and gave up waiting.
        let fetch = self.storage.is_dataless(path);
        let ours = fetch || self.host.fetched_earlier(path);
        // Told before the wait for room and the download, the slow part, so that the window names
        // the song they are for.
        self.host.progress(&DetailsProgress {
            read: self.summary.read,
            skipped: self.summary.skipped,
            missing: self.summary.missing,
            total,
            title: song.title.clone(),
            download: fetch,
        });
        if fetch {
            if !self.make_room(path, song.file_size)? {
                return Ok(Outcome::Stopped);
            }
            self.host.fetching(path);
            match self.fetch(path, song.file_size) {
                Fetch::Here => self.stalls = 0,
                Fetch::Failed => return Ok(Outcome::Skipped),
                Fetch::Stopped => return Ok(Outcome::Stopped),
                Fetch::Stalled => {
                    self.stalls += 1;
                    if self.stalls >= STALLS_BEFORE_OFFLINE {
                        self.stalls = 0;
                        if !self.host.blocked(Blocked::NoResponse) {
                            return Ok(Outcome::Stopped);
                        }
                    }
                    return Ok(Outcome::Skipped);
                }
            }
        }
        self.library.refresh_cloud_status(&[song.id])?;
        // Whether the details are in, not whether this call put them there: playback, a sync or
        // a rescan may have read the file since the batch was chosen.
        let local = !self.storage.is_dataless(path);
        let read = self.library.details_read(song.id)? && local;
        // A file that is a placeholder again was not given a fair try.
        if local {
            self.library.remember_unreadable(song.id, !read)?;
        }
        if read {
            // A cover that cannot be kept costs the song its picture, not its details.
            let _ = self.library.keep_cloud_cover(song.id);
        }
        // Owed even when the file would not read: it came down for this.
        if ours {
            self.owed.push(Fetched {
                id: song.id,
                path: path.to_path_buf(),
                released: false,
                refusals: 0,
            });
        }
        Ok(if read {
            Outcome::Read
        } else {
            Outcome::Skipped
        })
    }

    /// A song with no file. While its library (the first folder below the root) is there, the
    /// song was deleted, wherever in it, and is flagged. When it is not, iCloud Drive being off or
    /// the folder being renamed or away is likelier, and an empty container would otherwise have
    /// the whole library flagged, so the song is left as it is, to be found again with its
    /// library, and enough of them in a row end the read instead of passing over everything.
    fn lost(&mut self, song: &UnreadSong) -> Result<Outcome> {
        let library = self
            .root
            .and_then(|root| library_folder(root, Path::new(&song.path)));
        if library.is_some_and(|folder| folder.exists()) {
            self.library.mark_missing(song.id)?;
            return Ok(Outcome::Missing);
        }
        self.missing_in_a_row += 1;
        if self.missing_in_a_row >= MISSING_BEFORE_BLOCK {
            self.missing_in_a_row = 0;
            if !self.host.blocked(Blocked::FilesMissing) {
                return Ok(Outcome::Stopped);
            }
        }
        Ok(Outcome::Missing)
    }

    fn fetch(&self, path: &Path, size: u64) -> Fetch {
        let cancel = self.host.cancel_flag();
        let stop = AtomicBool::new(false);
        let limit = self
            .pacing
            .stall
            .max(Duration::from_secs(size / SLOWEST_DOWNLOAD));
        let mut latest = (Instant::now(), 0);
        let mut watch = |bytes: u64, _total: u64| {
            if bytes != latest.1 {
                latest = (Instant::now(), bytes);
            }
            if cancel.load(Ordering::Relaxed) || latest.0.elapsed() > limit {
                stop.store(true, Ordering::Relaxed);
            }
        };
        match wait_for_local(path, &mut watch, &stop, self.storage, self.pacing.poll) {
            Ok(()) => Fetch::Here,
            Err(Error::Cancelled) if cancel.load(Ordering::Relaxed) => Fetch::Stopped,
            Err(Error::Cancelled) => Fetch::Stalled,
            Err(_) => Fetch::Failed,
        }
    }

    /// Waits until the disk has room for the download, giving back what is owed to make it. False
    /// if the read was stopped meanwhile.
    fn make_room(&mut self, path: &Path, size: u64) -> Result<bool> {
        let needed = SPARE_BYTES.max(SPARE_PER_SONG * size);
        let mut waits = self.pacing.space_waits.iter();
        loop {
            self.settle()?;
            let free = self.host.free_bytes(path);
            if free >= needed {
                return Ok(true);
            }
            match waits.next() {
                Some(wait) => {
                    if !nap(self.host.cancel_flag(), *wait, self.pacing.poll) {
                        return Ok(false);
                    }
                }
                None => {
                    if !self.host.blocked(Blocked::LowSpace { free, needed }) {
                        return Ok(false);
                    }
                    waits = self.pacing.space_waits.iter();
                }
            }
        }
    }

    /// Gives back the files that are owed and can go, and tells the library about those that
    /// have become placeholders again.
    fn settle(&mut self) -> Result<()> {
        if self.owed.is_empty() {
            return Ok(());
        }
        let in_use = self.host.in_use();
        let storage = self.storage;
        let mut returned = Vec::new();
        let (mut accepted, mut dropped) = (false, 0);
        self.owed.retain_mut(|file| {
            if !file.released {
                // The listener has it: it goes back once they have moved on.
                if in_use.contains(&file.id) {
                    return true;
                }
                // The system declines a file it is not done with, and nothing is lost: the song
                // just stays on this machine, where the host still has it listed.
                if evict_with(&file.path, storage).is_err() {
                    file.refusals += 1;
                    if file.refusals < EVICTION_TRIES {
                        return true;
                    }
                    dropped += 1;
                    return false;
                }
                file.released = true;
                accepted = true;
            }
            if storage.is_dataless(&file.path) {
                returned.push((file.id, file.path.clone()));
                return false;
            }
            true
        });
        if accepted {
            self.refused = 0;
        }
        self.refused += dropped;
        if !returned.is_empty() {
            let ids: Vec<i64> = returned.iter().map(|(id, _)| *id).collect();
            self.library.refresh_cloud_status(&ids)?;
            for (_, path) in &returned {
                self.host.given_back(path);
            }
        }
        Ok(())
    }

    /// Gives back the last files, waiting a little for the system to take them: the library would
    /// otherwise show them as local until the next scan.
    fn give_back(&mut self) -> Result<()> {
        for _ in 0..self.pacing.finish_polls {
            self.settle()?;
            // A song still being played stays as it is; only returns already under way, and
            // refusals still being tried again, are waited for.
            if !self
                .owed
                .iter()
                .any(|file| file.released || file.refusals > 0)
            {
                break;
            }
            thread::sleep(self.pacing.poll);
        }
        Ok(())
    }
}

/// Sleeps for `total`, in pieces so that a stop is noticed. False if it was stopped.
fn nap(cancel: &AtomicBool, total: Duration, slice: Duration) -> bool {
    let mut left = total;
    while !left.is_zero() && !cancel.load(Ordering::Relaxed) {
        let step = left.min(slice);
        thread::sleep(step);
        left -= step;
    }
    !cancel.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::io;
    use std::sync::{Arc, Mutex};

    use tempfile::TempDir;

    use super::*;
    use crate::model::{Track, TrackQuery};

    /// Like the fake cloud in `cloud.rs`, but it knows which files are placeholders, so a run
    /// over several songs can be told what each did. A file given back becomes a placeholder
    /// only when `finish_evictions` says the system has got round to it, unless `instant`.
    #[derive(Default)]
    struct Drive {
        placeholders: Arc<Mutex<HashSet<PathBuf>>>,
        pending: Mutex<Vec<PathBuf>>,
        fetched: Mutex<Vec<PathBuf>>,
        evicted: Mutex<Vec<PathBuf>>,
        /// Downloads of these are asked for and never come, as when the Mac is offline.
        stuck: Mutex<HashSet<PathBuf>>,
        /// These cannot be asked for, and stay marked as placeholders even once they have been read through, as
        /// some providers leave them.
        unmarkable: Mutex<HashSet<PathBuf>>,
        /// How many times the system declines to take each back, as it does a file in use.
        refuse: Mutex<HashMap<PathBuf, u32>>,
        instant: bool,
    }

    impl Drive {
        fn instant() -> Self {
            Self {
                instant: true,
                ..Self::default()
            }
        }

        fn finish_evictions(&self) {
            let done: Vec<PathBuf> = self.pending.lock().unwrap().drain(..).collect();
            self.placeholders.lock().unwrap().extend(done);
        }

        fn fetched(&self) -> Vec<PathBuf> {
            self.fetched.lock().unwrap().clone()
        }

        fn evicted(&self) -> Vec<PathBuf> {
            self.evicted.lock().unwrap().clone()
        }
    }

    impl Storage for Drive {
        fn is_dataless(&self, path: &Path) -> bool {
            self.placeholders.lock().unwrap().contains(path)
        }

        fn start_download(&self, path: &Path) -> io::Result<()> {
            self.fetched.lock().unwrap().push(path.to_path_buf());
            if self.unmarkable.lock().unwrap().contains(path) {
                return Err(io::Error::other("no brctl"));
            }
            if !self.stuck.lock().unwrap().contains(path) {
                self.placeholders.lock().unwrap().remove(path);
            }
            Ok(())
        }

        fn evict(&self, path: &Path) -> io::Result<()> {
            if let Some(times) = self.refuse.lock().unwrap().get_mut(path)
                && *times > 0
            {
                *times -= 1;
                return Err(io::Error::other("in use"));
            }
            self.evicted.lock().unwrap().push(path.to_path_buf());
            if self.instant {
                self.placeholders.lock().unwrap().insert(path.to_path_buf());
            } else {
                self.pending.lock().unwrap().push(path.to_path_buf());
            }
            Ok(())
        }

        fn bytes_on_disk(&self, _path: &Path) -> u64 {
            0
        }
    }

    type Meanwhile = Box<dyn Fn()>;

    /// What the app around the read would say, and what it was told.
    struct Host {
        cancel: AtomicBool,
        in_use: RefCell<Vec<i64>>,
        /// The number of looks at the disk, from 1, to the bytes free at that look.
        free: Box<dyn Fn(u32) -> u64>,
        looks: Cell<u32>,
        /// How many songs it lets begin before it says it was stopped.
        turns: Cell<u32>,
        /// Whether the user, told the read is blocked, asks for another try.
        resumes: bool,
        /// The song being played is done with when this many songs have been announced.
        moves_on_at: u32,
        announced: Cell<u32>,
        /// Something that happens in the world just before each song is taken up.
        meanwhile: Option<Meanwhile>,
        blocked: RefCell<Vec<Blocked>>,
        titles: RefCell<Vec<String>>,
        /// For each song announced, whether it was about to be downloaded.
        downloads: RefCell<Vec<bool>>,
        changed: Cell<u32>,
        /// The files asked of the cloud and not yet given back, as the app keeps them.
        leftovers: RefCell<HashSet<PathBuf>>,
    }

    impl Host {
        fn new() -> Self {
            Host {
                cancel: AtomicBool::new(false),
                in_use: RefCell::default(),
                free: Box::new(|_| u64::MAX),
                looks: Cell::new(0),
                turns: Cell::new(u32::MAX),
                resumes: false,
                moves_on_at: u32::MAX,
                announced: Cell::new(0),
                meanwhile: None,
                blocked: RefCell::default(),
                titles: RefCell::default(),
                downloads: RefCell::default(),
                changed: Cell::new(0),
                leftovers: RefCell::default(),
            }
        }

        fn with_free(free: impl Fn(u32) -> u64 + 'static) -> Self {
            Host {
                free: Box::new(free),
                ..Self::new()
            }
        }
    }

    impl DetailsHost for Host {
        fn cancel_flag(&self) -> &AtomicBool {
            &self.cancel
        }

        fn wait_turn(&self) -> bool {
            if let Some(meanwhile) = &self.meanwhile {
                meanwhile();
            }
            let turns = self.turns.get();
            self.turns.set(turns.saturating_sub(1));
            turns > 0
        }

        fn free_bytes(&self, _path: &Path) -> u64 {
            self.looks.set(self.looks.get() + 1);
            (self.free)(self.looks.get())
        }

        fn in_use(&self) -> Vec<i64> {
            self.in_use.borrow().clone()
        }

        fn blocked(&self, reason: Blocked) -> bool {
            self.blocked.borrow_mut().push(reason);
            self.resumes
        }

        fn progress(&self, progress: &DetailsProgress) {
            self.announced.set(self.announced.get() + 1);
            if self.announced.get() == self.moves_on_at {
                self.in_use.borrow_mut().clear();
            }
            self.titles.borrow_mut().push(progress.title.clone());
            self.downloads.borrow_mut().push(progress.download);
        }

        fn changed(&self) {
            self.changed.set(self.changed.get() + 1);
        }

        fn fetching(&self, path: &Path) {
            self.leftovers.borrow_mut().insert(path.to_path_buf());
        }

        fn fetched_earlier(&self, path: &Path) -> bool {
            self.leftovers.borrow().contains(path)
        }

        fn given_back(&self, path: &Path) {
            self.leftovers.borrow_mut().remove(path);
        }
    }

    const WAITS: [Duration; 3] = [Duration::from_millis(1); 3];
    const ANYWHERE: &str = "";

    fn pacing() -> Pacing<'static> {
        Pacing {
            poll: Duration::from_millis(1),
            space_waits: &WAITS,
            finish_polls: 5,
            changed_every: 2,
            stall: Duration::from_millis(5),
        }
    }

    /// The fixtures a cloud holds, each filed as a scan would find it under Artist/Album/: one
    /// with a cover and Rock, one with Pop and no cover, and another of Rock.
    const SONGS: [(&str, &str); 3] = [
        (
            "come_together.flac",
            "The Beatles/Abbey Road/01 Come Together.flac",
        ),
        ("halo.mp3", "Beyonce/I Am/03 Halo.mp3"),
        ("something.m4a", "The Beatles/Abbey Road/02 Something.m4a"),
    ];

    /// Songs that are only in iCloud, as a scan finds them: real tagged files that the library
    /// knows by their names alone.
    struct Cloudy {
        music: TempDir,
        /// Where the songs live, still named once the folder is gone.
        root: PathBuf,
        data: TempDir,
        cache: TempDir,
        library: Library,
        drive: Arc<Drive>,
        paths: Vec<PathBuf>,
    }

    impl Cloudy {
        fn new(drive: Drive) -> Self {
            Self::of(drive, &SONGS)
        }

        /// `count` songs of the same fixture.
        fn copies(drive: Drive, count: usize) -> Self {
            let names: Vec<String> = (1..=count)
                .map(|n| format!("Artist/Album/{n:02} Song {n}.mp3"))
                .collect();
            let songs: Vec<(&str, &str)> = names.iter().map(|n| ("halo.mp3", n.as_str())).collect();
            Self::of(drive, &songs)
        }

        fn of(drive: Drive, songs: &[(&str, &str)]) -> Self {
            let (music, data, cache) = (TempDir::new(), TempDir::new(), TempDir::new());
            let (music, data, cache) = (music.unwrap(), data.unwrap(), cache.unwrap());
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
            let mut paths = Vec::new();
            for (fixture, relative) in songs {
                let path = music.path().join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::copy(fixtures.join(fixture), &path).unwrap();
                paths.push(fs::canonicalize(path).unwrap());
            }
            drive
                .placeholders
                .lock()
                .unwrap()
                .extend(paths.iter().cloned());
            let drive = Arc::new(drive);
            let mut cloudy = Cloudy {
                library: Library::open(&data.path().join("library.db")).unwrap(),
                root: fs::canonicalize(music.path()).unwrap(),
                music,
                data,
                cache,
                drive,
                paths,
            };
            cloudy.library = cloudy.connection();
            cloudy
                .library
                .add_paths(&[cloudy.music.path().to_path_buf()], |_| {})
                .unwrap();
            cloudy
        }

        /// Another connection to the library, as playback has.
        fn connection(&self) -> Library {
            let mut library = Library::open(&self.data.path().join("library.db")).unwrap();
            let placeholders = Arc::clone(&self.drive.placeholders);
            library.set_dataless_check(move |path| placeholders.lock().unwrap().contains(path));
            library.set_artwork_cache(self.cache.path().to_path_buf());
            library
        }

        fn track_at(&self, index: usize) -> Track {
            self.library
                .tracks(&TrackQuery {
                    kind: None,
                    ..TrackQuery::default()
                })
                .unwrap()
                .into_iter()
                .find(|track| Path::new(&track.path) == self.paths[index])
                .unwrap()
        }

        fn all_in_cloud(&self) -> bool {
            (0..self.paths.len()).all(|index| self.track_at(index).in_cloud)
        }

        fn read(&mut self, host: &Host) -> DetailsSummary {
            let root = self.root.clone();
            self.read_with(Path::new(ANYWHERE), Some(&root), host)
        }

        fn read_with(&mut self, within: &Path, root: Option<&Path>, host: &Host) -> DetailsSummary {
            let drive = Arc::clone(&self.drive);
            self.library
                .read_details_with(within, root, host, &*drive, &pacing())
                .unwrap()
        }

        /// The songs still waiting to be read.
        fn waiting(&self) -> u64 {
            self.library
                .unread_cloud_totals(Path::new(ANYWHERE))
                .unwrap()
                .songs
        }

        /// The first song a run reads, which is the one with the lowest id.
        fn first_id(&self) -> i64 {
            self.library
                .unread_cloud_songs(Path::new(ANYWHERE), 0, 1)
                .unwrap()[0]
                .id
        }
    }

    fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
        paths.sort();
        paths
    }

    fn summary(read: u64, skipped: u64, missing: u64) -> DetailsSummary {
        DetailsSummary {
            read,
            skipped,
            missing,
        }
    }

    #[test]
    fn songs_are_read_and_given_back() {
        let mut cloudy = Cloudy::new(Drive::instant());
        let before = cloudy
            .library
            .unread_cloud_totals(Path::new(ANYWHERE))
            .unwrap();
        assert_eq!(before.songs, 3);
        assert_eq!(
            before.bytes,
            cloudy
                .paths
                .iter()
                .map(|p| fs::metadata(p).unwrap().len())
                .sum::<u64>()
        );
        assert!(cloudy.track_at(0).genre.is_empty());
        let host = Host::new();

        let summary = cloudy.read(&host);

        assert_eq!(summary, super::tests::summary(3, 0, 0));
        let paths = sorted(cloudy.paths.clone());
        assert_eq!(sorted(cloudy.drive.fetched()), paths);
        assert_eq!(sorted(cloudy.drive.evicted()), paths);
        let (beatles, pop) = (cloudy.track_at(0), cloudy.track_at(1));
        assert_eq!(
            (beatles.genre.as_str(), pop.genre.as_str()),
            ("Rock", "Pop")
        );
        assert!(beatles.duration_ms > 0 && beatles.year == Some(1969));
        // The read ends with the files back in the cloud, and the library knowing it.
        assert!(cloudy.all_in_cloud());
        assert_eq!(cloudy.waiting(), 0);
        assert!(
            host.leftovers.borrow().is_empty(),
            "nothing is owed any more"
        );
        assert_eq!(host.titles.borrow().len(), 3);
        assert_eq!(
            *host.downloads.borrow(),
            [true; 3],
            "each is announced before its download"
        );
        // A batch of two, and the song left over at the end.
        assert_eq!(host.changed.get(), 2);
    }

    #[test]
    fn a_file_that_was_already_here_is_read_and_never_given_back() {
        let mut cloudy = Cloudy::new(Drive::instant());
        // Downloaded by the user since the scan.
        cloudy
            .drive
            .placeholders
            .lock()
            .unwrap()
            .remove(&cloudy.paths[1]);

        let host = Host::new();

        let summary = cloudy.read(&host);

        assert_eq!(summary.read, 3);
        assert_eq!(
            host.downloads
                .borrow()
                .iter()
                .filter(|download| **download)
                .count(),
            2,
            "only the others are announced as downloads"
        );
        assert!(!cloudy.drive.fetched().contains(&cloudy.paths[1]));
        assert!(!cloudy.drive.evicted().contains(&cloudy.paths[1]));
        let halo = cloudy.track_at(1);
        assert_eq!(halo.genre, "Pop");
        assert!(!halo.in_cloud, "it is here, so it is not flagged");
    }

    #[test]
    fn a_song_read_by_something_else_since_the_batch_was_chosen_still_counts_and_keeps_its_cover() {
        // The one song with a cover, chosen by the read already.
        let mut cloudy = Cloudy::of(Drive::instant(), &SONGS[..1]);
        let id = cloudy.track_at(0).id;
        // Playback downloads the song and reads it just as the read comes to it.
        let playback = RefCell::new(cloudy.connection());
        let placeholders = Arc::clone(&cloudy.drive.placeholders);
        let path = cloudy.paths[0].clone();
        let host = Host {
            meanwhile: Some(Box::new(move || {
                placeholders.lock().unwrap().remove(&path);
                playback.borrow_mut().refresh_cloud_status(&[id]).unwrap();
            })),
            ..Host::new()
        };

        let summary = cloudy.read(&host);

        assert_eq!(summary, super::tests::summary(1, 0, 0));
        assert!(cloudy.library.cloud_cover(&cloudy.track_at(0)).is_some());
        // Not the read's to give back: it was not fetched by it.
        assert!(!cloudy.drive.fetched().contains(&cloudy.paths[0]));
        assert!(!cloudy.drive.evicted().contains(&cloudy.paths[0]));
        assert!(!cloudy.track_at(0).in_cloud);
    }

    #[test]
    fn the_song_that_is_playing_or_next_is_never_given_back() {
        let mut cloudy = Cloudy::new(Drive::instant());
        let playing = cloudy.first_id();
        let playing_path = PathBuf::from(cloudy.library.track(playing).unwrap().unwrap().path);
        let host = Host::new();
        host.in_use.borrow_mut().push(playing);

        let summary = cloudy.read(&host);

        assert_eq!(summary.read, 3);
        assert_eq!(cloudy.drive.fetched().len(), 3);
        assert!(!cloudy.drive.evicted().contains(&playing_path));
        assert_eq!(cloudy.drive.evicted().len(), 2);
        let track = cloudy.library.track(playing).unwrap().unwrap();
        assert!(
            !track.in_cloud && !track.genre.is_empty(),
            "read, and still here"
        );
        // Still owed, for the app to give back when the song is done with.
        assert_eq!(*host.leftovers.borrow(), HashSet::from([playing_path]));
    }

    #[test]
    fn a_song_held_back_for_playback_goes_back_once_playback_has_moved_on() {
        let mut cloudy = Cloudy::new(Drive::instant());
        let playing = cloudy.first_id();
        let playing_path = PathBuf::from(cloudy.library.track(playing).unwrap().unwrap().path);
        let host = Host {
            moves_on_at: 3,
            ..Host::new()
        };
        host.in_use.borrow_mut().push(playing);

        cloudy.read(&host);

        // Held through the second song's turn, given back on the third's.
        assert_eq!(
            cloudy
                .drive
                .evicted()
                .iter()
                .position(|p| *p == playing_path),
            Some(1)
        );
        assert!(cloudy.all_in_cloud());
        assert!(host.leftovers.borrow().is_empty());
    }

    #[test]
    fn a_stopped_read_carries_on_where_it_left_off() {
        let mut cloudy = Cloudy::new(Drive::instant());
        let first = Host {
            turns: Cell::new(1),
            ..Host::new()
        };

        let summary = cloudy.read(&first);

        assert_eq!(summary.read, 1);
        assert_eq!(cloudy.waiting(), 2);

        // Another run, as after a restart: the song read is not fetched again.
        let summary = cloudy.read(&Host::new());

        assert_eq!(summary.read, 2);
        assert_eq!(
            sorted(cloudy.drive.fetched()),
            sorted(cloudy.paths.clone()),
            "each song was fetched once"
        );
        assert_eq!(cloudy.waiting(), 0);
    }

    #[test]
    fn a_song_that_was_read_by_an_older_parser_is_not_downloaded_again() {
        let mut cloudy = Cloudy::new(Drive::instant());
        let id = cloudy.first_id();
        cloudy
            .library
            .conn
            .execute("UPDATE tracks SET parser_version = 1 WHERE id = ?1", [id])
            .unwrap();

        let summary = cloudy.read(&Host::new());

        assert_eq!(summary.read, 2);
        assert_eq!(cloudy.drive.fetched().len(), 2);
    }

    #[test]
    fn only_the_songs_inside_the_place_it_works_in_are_read() {
        let mut cloudy = Cloudy::new(Drive::instant());
        let beatles = fs::canonicalize(cloudy.music.path())
            .unwrap()
            .join("The Beatles")
            .join("");
        assert_eq!(
            cloudy.library.unread_cloud_totals(&beatles).unwrap().songs,
            2
        );

        let summary = cloudy.read_with(&beatles, None, &Host::new());

        assert_eq!(summary.read, 2);
        assert!(!cloudy.drive.fetched().contains(&cloudy.paths[1]));
        assert_eq!(cloudy.waiting(), 1);
    }

    #[test]
    fn nothing_is_fetched_while_the_disk_is_nearly_full_and_the_read_asks_what_to_do() {
        let mut cloudy = Cloudy::new(Drive::instant());
        let host = Host::with_free(|_| 1 << 30);

        let summary = cloudy.read(&host);

        assert_eq!(summary, DetailsSummary::default());
        assert!(cloudy.drive.fetched().is_empty());
        assert_eq!(
            *host.blocked.borrow(),
            [Blocked::LowSpace {
                free: 1 << 30,
                needed: SPARE_BYTES
            }]
        );
        // A look at the disk before each wait and one after the last, not one for each wait.
        assert_eq!(host.looks.get(), WAITS.len() as u32 + 1);
    }

    #[test]
    fn a_big_song_needs_three_times_its_size_free() {
        let mut cloudy = Cloudy::new(Drive::instant());
        let id = cloudy.first_id();
        cloudy
            .library
            .conn
            .execute(
                "UPDATE tracks SET file_size = ?2 WHERE id = ?1",
                params![id, 20u64 << 30],
            )
            .unwrap();
        let host = Host::with_free(|_| 40 << 30);

        cloudy.read(&host);

        assert_eq!(
            *host.blocked.borrow(),
            [Blocked::LowSpace {
                free: 40 << 30,
                needed: 60 << 30
            }]
        );
    }

    #[test]
    fn a_read_waits_for_what_it_gave_back_to_leave_before_the_next_download() {
        let mut cloudy = Cloudy::new(Drive::default());
        let drive = Arc::clone(&cloudy.drive);
        // There is room only once the system has taken the last song back, which it does on
        // the third look at the disk after the song was given.
        let host = Host::with_free(move |looks| {
            if drive.pending.lock().unwrap().is_empty() {
                return u64::MAX;
            }
            if looks % 3 == 0 {
                drive.finish_evictions();
            }
            1 << 30
        });

        // This tests eviction/space pacing, not download stalls. The shared 5 ms test
        // deadline can expire during the initial file stat on a busy Windows runner,
        // before the fake drive gets to complete its synchronous download.
        let pacing = Pacing {
            stall: Duration::MAX,
            ..pacing()
        };
        let summary = cloudy
            .library
            .read_details_with(
                Path::new(ANYWHERE),
                Some(&cloudy.root),
                &host,
                &*cloudy.drive,
                &pacing,
            )
            .unwrap();

        assert_eq!(summary, super::tests::summary(3, 0, 0));
        assert!(host.looks.get() > 3, "it did have to wait");
        assert!(host.blocked.borrow().is_empty());
        // The system never took the last one back while the read waited, so the library still
        // shows it as local.
        let flagged = (0..cloudy.paths.len()).filter(|i| cloudy.track_at(*i).in_cloud);
        assert_eq!(flagged.count(), 2);
    }

    #[test]
    fn a_read_the_user_lets_go_on_after_a_full_disk_tries_again() {
        let mut cloudy = Cloudy::new(Drive::instant());
        // Full for every wait and the look after the last, then free.
        let host = Host {
            resumes: true,
            ..Host::with_free(|looks| {
                if looks <= WAITS.len() as u32 + 1 {
                    1 << 30
                } else {
                    u64::MAX
                }
            })
        };

        let summary = cloudy.read(&host);

        assert_eq!(summary.read, 3);
        assert_eq!(host.blocked.borrow().len(), 1);
    }

    #[test]
    fn downloads_that_bring_in_nothing_are_skipped_and_three_in_a_row_stop_the_read_to_ask() {
        let mut cloudy = Cloudy::new(Drive::instant());
        cloudy
            .drive
            .stuck
            .lock()
            .unwrap()
            .extend(cloudy.paths.iter().cloned());
        let host = Host::new();

        let summary = cloudy.read(&host);

        // The third was not given up on: the read was stopped over it.
        assert_eq!(summary, super::tests::summary(0, 2, 0));
        assert_eq!(*host.blocked.borrow(), [Blocked::NoResponse]);
        assert_eq!(cloudy.waiting(), 3);
        // Every one was asked of the cloud and none came, so the app still has them listed: it
        // gives them back if they do.
        assert_eq!(
            *host.leftovers.borrow(),
            cloudy.paths.iter().cloned().collect()
        );
    }

    #[test]
    fn two_stalled_downloads_do_not_stop_the_rest() {
        let mut cloudy = Cloudy::new(Drive::instant());
        cloudy
            .drive
            .stuck
            .lock()
            .unwrap()
            .extend(cloudy.paths[..2].iter().cloned());
        let host = Host::new();

        let summary = cloudy.read(&host);

        assert_eq!(summary, super::tests::summary(1, 2, 0));
        assert!(host.blocked.borrow().is_empty());
    }

    #[test]
    fn a_file_that_arrived_after_an_earlier_read_gave_up_is_given_back_with_the_rest() {
        let mut cloudy = Cloudy::new(Drive::instant());
        // It came down on its own, after the read that asked for it had stopped waiting.
        let late = cloudy.paths[1].clone();
        cloudy.drive.placeholders.lock().unwrap().remove(&late);
        let host = Host::new();
        host.leftovers.borrow_mut().insert(late.clone());

        let summary = cloudy.read(&host);

        assert_eq!(summary.read, 3);
        assert!(
            !cloudy.drive.fetched().contains(&late),
            "it was here already"
        );
        assert!(cloudy.drive.evicted().contains(&late));
        assert!(host.leftovers.borrow().is_empty());
        assert!(cloudy.track_at(1).in_cloud);
    }

    #[test]
    fn a_song_that_is_declined_is_asked_again_and_then_left_listed() {
        let mut cloudy = Cloudy::new(Drive::instant());
        // Busy for two asks, as when something else has the file open for a moment.
        cloudy
            .drive
            .refuse
            .lock()
            .unwrap()
            .insert(cloudy.paths[0].clone(), 2);
        let host = Host::new();

        cloudy.read(&host);

        assert_eq!(sorted(cloudy.drive.evicted()), sorted(cloudy.paths.clone()));
        assert!(cloudy.all_in_cloud());
        assert!(host.blocked.borrow().is_empty());

        let mut stubborn = Cloudy::new(Drive::instant());
        stubborn
            .drive
            .refuse
            .lock()
            .unwrap()
            .insert(stubborn.paths[0].clone(), u32::MAX);
        let host = Host::new();

        stubborn.read(&host);

        assert!(!stubborn.drive.evicted().contains(&stubborn.paths[0]));
        assert_eq!(
            *host.leftovers.borrow(),
            HashSet::from([stubborn.paths[0].clone()])
        );
        assert!(!stubborn.track_at(0).in_cloud, "it is here, and says so");
    }

    #[test]
    fn songs_that_keep_being_declined_stop_the_read_before_the_disk_fills() {
        let mut cloudy = Cloudy::copies(Drive::instant(), 7);
        cloudy
            .drive
            .refuse
            .lock()
            .unwrap()
            .extend(cloudy.paths.iter().map(|path| (path.clone(), u32::MAX)));
        let host = Host::new();

        let summary = cloudy.read(&host);

        assert_eq!(*host.blocked.borrow(), [Blocked::NotGivenBack]);
        assert_eq!(summary.read, 4);
        assert_eq!(cloudy.drive.fetched().len(), 4);
    }

    #[test]
    fn a_stop_while_waiting_for_space_ends_the_read() {
        let mut cloudy = Cloudy::new(Drive::instant());
        let host = Host::with_free(|_| 0);
        host.cancel.store(true, Ordering::Relaxed);

        let summary = cloudy.read(&host);

        assert_eq!(summary, DetailsSummary::default());
        assert!(host.blocked.borrow().is_empty());
    }

    #[test]
    fn a_song_whose_file_is_gone_is_flagged_missing_and_not_counted_as_skipped() {
        let mut cloudy = Cloudy::new(Drive::instant());
        fs::remove_file(&cloudy.paths[0]).unwrap();
        let host = Host::new();

        let summary = cloudy.read(&host);

        assert_eq!(summary, super::tests::summary(2, 0, 1));
        assert!(cloudy.track_at(0).missing);
        assert!(!cloudy.drive.fetched().contains(&cloudy.paths[0]));
        assert_eq!(cloudy.waiting(), 0, "it is not asked for again");
    }

    #[test]
    fn a_song_in_a_deleted_folder_is_flagged_missing_too() {
        let mut cloudy = Cloudy::new(Drive::instant());
        fs::remove_dir_all(cloudy.paths[1].parent().unwrap()).unwrap();

        let summary = cloudy.read(&Host::new());

        assert_eq!(summary, super::tests::summary(2, 0, 1));
        assert!(cloudy.track_at(1).missing);
        assert_eq!(cloudy.waiting(), 0);
    }

    #[test]
    fn many_files_deleted_from_a_root_that_is_there_never_pause_the_read() {
        let mut cloudy = Cloudy::copies(Drive::instant(), 8);
        for path in &cloudy.paths {
            fs::remove_file(path).unwrap();
        }
        let host = Host::new();

        let summary = cloudy.read(&host);

        assert!(host.blocked.borrow().is_empty());
        assert_eq!(summary, super::tests::summary(0, 0, 8));
        assert!((0..8).all(|index| cloudy.track_at(index).missing));
        assert_eq!(cloudy.waiting(), 0);
        assert_eq!(host.changed.get(), 1, "the window is told what was flagged");
    }

    #[test]
    fn a_renamed_library_folder_in_a_container_that_is_there_pauses_the_read_and_flags_nothing() {
        let mut cloudy = Cloudy::copies(Drive::instant(), 7);
        let root = cloudy.root.clone();
        assert!(
            cloudy
                .library
                .unread_cloud_library_there(Path::new(ANYWHERE), Some(&root))
                .unwrap()
        );
        // The container is there, as iCloud Drive's is even when it is empty, but not the Music folder in it.
        fs::rename(root.join("Artist"), root.join("Renamed")).unwrap();
        assert!(
            !cloudy
                .library
                .unread_cloud_library_there(Path::new(ANYWHERE), Some(&root))
                .unwrap()
        );
        let host = Host::new();

        let summary = cloudy.read(&host);

        assert_eq!(*host.blocked.borrow(), [Blocked::FilesMissing]);
        assert_eq!(summary.missing, 4);
        assert_eq!(cloudy.waiting(), 7, "none is blamed for it");
        assert!((0..7).all(|index| !cloudy.track_at(index).missing));
    }

    #[test]
    fn a_songs_library_is_the_first_folder_below_the_root() {
        let root = Path::new("/cloud/Drive");
        let library = |song: &str| library_folder(root, Path::new(song));
        assert_eq!(
            library("/cloud/Drive/Music/Artist/Album/Song.mp3"),
            Some(PathBuf::from("/cloud/Drive/Music"))
        );
        assert_eq!(
            library("/cloud/Drive/Music/Song.mp3"),
            Some(PathBuf::from("/cloud/Drive/Music"))
        );
        assert_eq!(
            library("/cloud/Drive/Song.mp3"),
            Some(PathBuf::from("/cloud/Drive"))
        );
        assert_eq!(library("/elsewhere/Music/Song.mp3"), None);
    }

    #[test]
    fn five_songs_in_a_row_with_the_root_gone_too_stop_the_read_to_ask() {
        let mut cloudy = Cloudy::copies(Drive::instant(), 7);
        // iCloud Drive turned off, or the folder moved: they may well be back tomorrow.
        fs::remove_dir_all(&cloudy.root).unwrap();
        let host = Host::new();

        let summary = cloudy.read(&host);

        assert_eq!(*host.blocked.borrow(), [Blocked::FilesMissing]);
        assert_eq!(summary.missing, 4, "the fifth was not given up on");
        assert!(cloudy.drive.fetched().is_empty());
        assert_eq!(cloudy.waiting(), 7, "none is blamed for it");
        assert!(!cloudy.track_at(0).missing);
    }

    #[test]
    fn without_a_root_to_go_by_a_song_found_between_missing_ones_starts_the_count_again() {
        let mut cloudy = Cloudy::copies(Drive::instant(), 9);
        // In the order they are read: four gone, one here, four gone. Never five in a row.
        let order: Vec<String> = cloudy
            .library
            .unread_cloud_songs(Path::new(ANYWHERE), 0, 100)
            .unwrap()
            .into_iter()
            .map(|song| song.path)
            .collect();
        for path in order
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 4)
            .map(|(_, p)| p)
        {
            fs::remove_file(path).unwrap();
        }
        let host = Host::new();

        let summary = cloudy.read_with(Path::new(ANYWHERE), None, &host);

        assert!(host.blocked.borrow().is_empty());
        assert_eq!(summary, super::tests::summary(1, 0, 8));
        assert!(
            (0..9).all(|index| !cloudy.track_at(index).missing),
            "with no root to say it is there, no song is blamed"
        );
    }

    #[test]
    fn a_file_that_is_still_a_placeholder_after_the_download_is_not_noted_as_unreadable() {
        let mut cloudy = Cloudy::new(Drive::instant());
        cloudy
            .drive
            .unmarkable
            .lock()
            .unwrap()
            .insert(cloudy.paths[1].clone());

        let summary = cloudy.read(&Host::new());

        assert_eq!(summary, super::tests::summary(2, 1, 0));
        assert_eq!(
            cloudy.waiting(),
            1,
            "it will be tried again, not written off"
        );
    }

    #[test]
    fn a_song_that_comes_down_unreadable_is_not_downloaded_again_until_its_file_changes() {
        let mut cloudy = Cloudy::new(Drive::instant());
        fs::write(&cloudy.paths[1], b"not a song").unwrap();

        let first = cloudy.read(&Host::new());

        assert_eq!(first, super::tests::summary(2, 1, 0));
        // The unreadable file did come down for this, so it goes back all the same.
        assert!(cloudy.drive.evicted().contains(&cloudy.paths[1]));
        assert_eq!(cloudy.track_at(2).genre, "Rock");

        let again = cloudy.read(&Host::new());

        assert_eq!(again, DetailsSummary::default());
        assert_eq!(cloudy.drive.fetched().len(), 3, "not fetched a second time");
        assert_eq!(cloudy.waiting(), 0);

        // iCloud brings a new version of the file.
        let halo = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/halo.mp3");
        fs::copy(halo, &cloudy.paths[1]).unwrap();
        cloudy
            .library
            .conn
            .execute(
                "UPDATE tracks SET date_modified = date_modified + 1 WHERE path = ?1",
                [cloudy.paths[1].to_str().unwrap()],
            )
            .unwrap();

        let changed = cloudy.read(&Host::new());

        assert_eq!(changed, super::tests::summary(1, 0, 0));
        assert_eq!(cloudy.drive.fetched().len(), 4);
        assert_eq!(cloudy.track_at(1).genre, "Pop");
    }

    #[test]
    fn the_cover_is_served_from_the_store_once_the_file_is_back_in_the_cloud() {
        let mut cloudy = Cloudy::new(Drive::instant());
        let beatles = cloudy.track_at(0);
        assert!(cloudy.library.artwork(beatles.id).unwrap().is_none());

        cloudy.read(&Host::new());

        // Nothing but the store can give it now: even the placeholder is gone.
        fs::remove_file(&cloudy.paths[0]).unwrap();
        let cover = cloudy.library.artwork(beatles.id).unwrap().unwrap();
        assert_eq!(cover.mime, "image/jpeg");
        let thumbnail = cloudy
            .library
            .artwork_thumbnail(beatles.id, 8)
            .unwrap()
            .unwrap();
        assert_eq!(
            image::load_from_memory(&thumbnail.bytes).unwrap().width(),
            8
        );
        // A song with no picture of its own has none to show.
        assert!(
            cloudy
                .library
                .artwork(cloudy.track_at(1).id)
                .unwrap()
                .is_none()
        );
        // The cover is kept with the library's data, and only what is made from it is cached.
        assert!(cloudy.data.path().join("Covers").is_dir());
        assert!(cloudy.cache.path().join("Cloud").is_dir());
        assert!(
            fs::read_dir(cloudy.cache.path().join("Cloud"))
                .unwrap()
                .all(|entry| entry.unwrap().file_name().to_string_lossy().contains("-8."))
        );
    }

    #[test]
    fn the_wish_to_carry_on_is_remembered() {
        let mut library = Library::open_in_memory().unwrap();
        assert!(!library.cloud_details_wanted().unwrap());

        library.set_cloud_details_wanted(true).unwrap();
        assert!(library.cloud_details_wanted().unwrap());

        library.set_cloud_details_wanted(false).unwrap();
        assert!(!library.cloud_details_wanted().unwrap());
    }

    #[test]
    fn a_nap_ends_early_when_stopped() {
        let started = Instant::now();
        let stopped = AtomicBool::new(true);

        assert!(!nap(
            &stopped,
            Duration::from_secs(60),
            Duration::from_millis(5)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(nap(
            &AtomicBool::new(false),
            Duration::from_millis(3),
            Duration::from_millis(1)
        ));
    }
}
