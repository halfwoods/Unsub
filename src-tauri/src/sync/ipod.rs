use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::mem::discriminant;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use unsub_ipod::{Ipod, IpodMediaKind, Prepared, SyncPlaylist, SyncProgress, SyncStage, SyncSupport, SyncTrack};
use unsub_library::{MediaKind, Track};
use unsub_playback::{Target, TranscodeOptions};

use super::cloud::{Cloud, Covers, ICloud, giving_back, leftovers_of};
use super::{Chosen, FALLBACK_AAC_KBPS, LOSSY_CODECS, SyncState, fresh_work_dir, is_lossless, select, to_error};
use crate::devices::{self, DevicesState};
use crate::error::{CommandError, CommandResult};
use crate::jobs::Ctx;
use crate::library::{self, LibraryState};
use crate::playback::{PlaySource, PlaybackState};

/// iPods play Apple Lossless up to 48 kHz and 16 bits.
const IPOD_MAX_SAMPLE_RATE: u32 = 48_000;
const IPOD_MAX_BIT_DEPTH: u8 = 16;
const IPOD_CODECS: [&str; 5] = ["AAC", "MP3", "ALAC", "WAV", "AIFF"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Conversion {
    Copy,
    Alac,
    Aac(u32),
}

impl Conversion {
    /// Decided from the library's details so a preview needn't open thousands of files; the
    /// actual file is checked again just before copying.
    fn for_track(track: &Track, convert_kbps: Option<u32>) -> Self {
        let lossless = !LOSSY_CODECS.contains(&track.codec.as_str());
        if let Some(kbps) = convert_kbps
            && (lossless || track.bitrate_kbps.is_some_and(|rate| rate > kbps))
        {
            return Self::Aac(kbps);
        }
        let fits = track.sample_rate.is_none_or(|rate| rate <= IPOD_MAX_SAMPLE_RATE)
            && track.bit_depth.is_none_or(|bits| bits <= IPOD_MAX_BIT_DEPTH)
            && track.channels.is_none_or(|channels| channels <= 2);
        if IPOD_CODECS.contains(&track.codec.as_str()) && fits {
            Self::Copy
        } else if lossless {
            Self::Alac
        } else {
            Self::Aac(FALLBACK_AAC_KBPS)
        }
    }
}

/// The rate a converted file ends up at: the transcoder stays in the source's 44.1 or 48 kHz family.
fn ipod_rate(rate: u32) -> u32 {
    if rate <= IPOD_MAX_SAMPLE_RATE {
        rate
    } else if rate.is_multiple_of(44_100) {
        44_100
    } else {
        IPOD_MAX_SAMPLE_RATE
    }
}

/// The iPod's view of a selection: songs as its database describes them, and how each file is
/// made playable (the conversion, and whether the source is lossless).
pub(super) struct IpodSelection {
    pub(super) tracks: Vec<SyncTrack>,
    pub(super) conversions: HashMap<i64, (Conversion, bool)>,
    pub(super) playlists: Vec<SyncPlaylist>,
    /// The songs the library has down as only in iCloud, whose details are a placeholder's.
    pub(super) in_cloud: HashSet<i64>,
}

pub(super) fn for_ipod(chosen: &Chosen, convert_kbps: Option<u32>) -> IpodSelection {
    let mut selection = IpodSelection { tracks: Vec::new(), conversions: HashMap::new(), playlists: Vec::new(), in_cloud: HashSet::new() };
    for track in &chosen.tracks {
        if track.in_cloud {
            selection.in_cloud.insert(track.id);
        }
        let conversion = Conversion::for_track(track, convert_kbps);
        selection.conversions.insert(track.id, (conversion, is_lossless(track)));
        selection.tracks.push(sync_track(track, conversion));
    }
    selection.playlists =
        chosen.playlists.iter().map(|(name, tracks)| SyncPlaylist { name: name.clone(), tracks: tracks.clone() }).collect();
    selection
}

fn sync_track(track: &Track, conversion: Conversion) -> SyncTrack {
    let (bitrate_kbps, sample_rate) = match conversion {
        Conversion::Copy => (track.bitrate_kbps, track.sample_rate),
        Conversion::Alac => (None, track.sample_rate.map(ipod_rate)),
        Conversion::Aac(kbps) => (Some(kbps), track.sample_rate.map(ipod_rate)),
    };
    SyncTrack {
        source_path: PathBuf::from(&track.path),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album_artist: track.album_artist.clone(),
        album: track.album.clone(),
        genre: track.genre.clone(),
        composer: track.composer.clone(),
        year: track.year,
        track_number: track.track_number,
        track_count: track.track_count,
        disc_number: track.disc_number,
        disc_count: track.disc_count,
        duration_ms: track.duration_ms,
        bitrate_kbps,
        sample_rate,
        rating: track.rating,
        play_count: track.play_count,
        last_played: track.last_played,
        date_added: track.date_added,
        media_kind: match track.kind {
            MediaKind::Music => IpodMediaKind::Music,
            MediaKind::Podcast => IpodMediaKind::Podcast,
            MediaKind::Audiobook => IpodMediaKind::Audiobook,
            // `select` leaves stations out, so none reaches here.
            MediaKind::Radio => IpodMediaKind::Other,
        },
        compilation: track.compilation,
        sort_title: track.sort_title.clone(),
        sort_artist: track.sort_artist.clone(),
        sort_album: track.sort_album.clone(),
        sort_album_artist: track.sort_album_artist.clone(),
        artwork_key: Some(artwork_key(track)),
        // iTunes always wrote Sound Check values; the iPod's own setting decides whether to use them.
        sound_check: track.loudness.as_ref().map(|l| unsub_library::itunes_soundcheck(crate::playback::SOUND_CHECK_LUFS - l.lufs)),
        library_id: track.id,
    }
}

/// Covers live in the files (Get Info rewrites the file to change one), so the file's identity
/// is the cover's: unchanged files are never re-read for artwork on later syncs. A Rust update
/// that changes the hasher only costs one round of re-sent covers.
fn artwork_key(track: &Track) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (&track.path, track.file_size, track.date_modified).hash(&mut hasher);
    hasher.finish()
}

/// The iPod as it is now: free space changes as music is added outside the app.
pub(super) fn fresh_ipod(devices: &DevicesState, device_id: &str) -> CommandResult<Ipod> {
    let ipod = devices.ipod(device_id)?;
    let ipod = unsub_ipod::open(&ipod.mount_path).map_err(to_error)?;
    if !matches!(ipod.sync, SyncSupport::Supported | SyncSupport::NeedsHash58) {
        return Err(CommandError::from("Unsub can't write to this iPod model's database yet".to_owned()));
    }
    Ok(ipod)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncStatus {
    device_id: String,
    #[serde(flatten)]
    progress: SyncProgress,
}

/// How long a download may go without a new byte before the song is left out, at least. A Mac that
/// is offline never finishes a download or fails it, so without a limit the sync would wait for
/// ever; and since how much of a file is on disk may not move until it is all there, a big file
/// (an audiobook) is given longer, as if it came at [`SLOWEST_DOWNLOAD`].
const DOWNLOAD_STALL: Duration = Duration::from_secs(180);
/// Bytes a second, below which a download is not expected to be going.
const SLOWEST_DOWNLOAD: u64 = 256 * 1024;
/// Stalled downloads in a row after which none is tried.
const TIMEOUTS_BEFORE_OFFLINE: u8 = 3;
const TIMED_OUT: &str = "timed out waiting for iCloud";
const NO_RESPONSE: &str = "skipped because iCloud isn't responding";
const UNREADABLE: &str = "couldn't be read once downloaded";
const NOT_CONVERTED: &str = "couldn't be converted for the iPod";
/// Covers kept for the iPod are no wider than this; its screens show them far smaller.
const COVER_PIXELS: u32 = 640;
/// How many songs' cloud status the library refreshes at once, so it is never held for long.
const REFRESH_BATCH: usize = 500;

fn stall_limit(floor: Duration, total_bytes: u64) -> Duration {
    floor.max(Duration::from_secs(total_bytes / SLOWEST_DOWNLOAD))
}

/// What an iPod sync's songs go through just before each is copied: a song that is only in iCloud
/// is downloaded and read, then (if asked) given back to iCloud once it has been copied, so the
/// Mac holds about one song of the library at a time. A song that can't be had is left out.
struct IpodSongs<'a> {
    library: &'a LibraryState,
    cloud: &'a dyn Cloud,
    covers: &'a Covers,
    emit: &'a dyn Fn(SyncProgress),
    /// (done, total) as `apply_sync` last reported them for the song being prepared.
    position: &'a Cell<(u64, u64)>,
    conversions: &'a HashMap<i64, (Conversion, bool)>,
    /// Songs the library has down as only in iCloud. One that is here after all, downloaded some
    /// other way since, is only read.
    in_cloud: &'a HashSet<i64>,
    convert_kbps: Option<u32>,
    work_dir: &'a Path,
    cancel: &'a AtomicBool,
    /// Give each downloaded song back to iCloud after it is copied.
    release: bool,
    /// Whether the iPod gets album art, so whether a downloaded song's cover is worth keeping.
    artwork: bool,
    stall: Duration,
    /// Downloads in a row that stalled. Enough of them and iCloud is taken to be out of reach,
    /// and the rest are not tried.
    timeouts: u8,
    /// The converted file of the song just prepared. `apply_sync` copies each prepared file before
    /// asking for the next, so it goes as soon as the next is requested.
    converted: Option<PathBuf>,
    /// The song just prepared, if this sync downloaded it and is to give it back.
    downloaded: Option<PathBuf>,
    /// Downloads this sync asked for that did not finish, because it was stopped or gave up on
    /// them. iCloud may yet complete them, and then they stay on the Mac unless given back.
    abandoned: Vec<PathBuf>,
    /// Library ids of the songs that came down, whose cloud status needs refreshing at the end.
    fetched: Vec<i64>,
    /// Why each song that was left out was.
    skipped: Vec<String>,
}

/// What `IpodSongs` leaves behind when the sync is over.
struct Fetched {
    songs: usize,
    skipped: Vec<String>,
    /// Downloads still going that the sync asked for, for [`Leftovers`].
    unfinished: Vec<PathBuf>,
}

/// How a song's download ended.
enum Fetch {
    Here,
    /// It can't be had, and the sync goes on without it.
    Left,
    /// The user stopped the sync.
    Stop,
}

impl IpodSongs<'_> {
    fn prepare(&mut self, track: &SyncTrack) -> Prepared {
        self.release_downloaded();
        if let Some(done) = self.converted.take() {
            let _ = std::fs::remove_file(done);
        }
        if self.cancel.load(Ordering::Relaxed) {
            return Prepared::Stop;
        }

        let placeholder = self.cloud.is_placeholder(&track.source_path);
        let fetched = if placeholder || self.in_cloud.contains(&track.library_id) {
            if placeholder {
                match self.download(track) {
                    Fetch::Here => {}
                    Fetch::Left => return Prepared::Skip,
                    Fetch::Stop => return Prepared::Stop,
                }
            }
            match self.read(track) {
                Some(song) => Some(song),
                None => return Prepared::Skip,
            }
        } else {
            None
        };
        // A song that came down is described by its file, not by the placeholder the library had.
        let (conversion, lossless, described) = match fetched {
            Some(song) => {
                let conversion = Conversion::for_track(&song, self.convert_kbps);
                (conversion, is_lossless(&song), Some(sync_track(&song, conversion)))
            }
            None => {
                let (conversion, lossless) = self.conversions.get(&track.library_id).copied().unwrap_or((Conversion::Copy, false));
                (conversion, lossless, None)
            }
        };
        match playable_file(described.as_ref().unwrap_or(track), conversion, lossless, self.work_dir, self.cancel) {
            Ok(path) => {
                if path != track.source_path {
                    self.converted = Some(path.clone());
                }
                Prepared::File { path, track: described.map(Box::new) }
            }
            // A stopped conversion fails like any other; one song that won't convert must not end
            // a sync that may be hours in.
            Err(_) if self.cancel.load(Ordering::Relaxed) => Prepared::Stop,
            Err(_) => {
                self.skipped.push(NOT_CONVERTED.to_owned());
                Prepared::Skip
            }
        }
    }

    fn download(&mut self, track: &SyncTrack) -> Fetch {
        if self.timeouts >= TIMEOUTS_BEFORE_OFFLINE {
            self.skipped.push(NO_RESPONSE.to_owned());
            return Fetch::Left;
        }
        let (done, total) = self.position.get();
        (self.emit)(SyncProgress { stage: SyncStage::Downloading, done, total, current: track.title.clone() });
        let (cancel, floor) = (self.cancel, self.stall);
        let stop = AtomicBool::new(false);
        let mut last = (Instant::now(), 0);
        let mut watch = |bytes: u64, total: u64| {
            if bytes != last.1 {
                last = (Instant::now(), bytes);
            }
            if cancel.load(Ordering::Relaxed) || last.0.elapsed() > stall_limit(floor, total) {
                stop.store(true, Ordering::Relaxed);
            }
        };
        match self.cloud.download(&track.source_path, &mut watch, &stop) {
            Ok(()) => {
                self.timeouts = 0;
                if self.release {
                    self.downloaded = Some(track.source_path.clone());
                }
                Fetch::Here
            }
            Err(error) => {
                // Giving up on a download does not call off the request, which iCloud may still
                // answer later. A download that failed outright has no request left.
                let waiting = matches!(error, unsub_library::Error::Cancelled);
                if waiting && self.release {
                    self.abandoned.push(track.source_path.clone());
                }
                match error {
                    unsub_library::Error::Cancelled if cancel.load(Ordering::Relaxed) => Fetch::Stop,
                    // Not the user's Stop: the watch above gave up on a download that stopped moving.
                    unsub_library::Error::Cancelled => {
                        self.timeouts += 1;
                        self.skipped.push(TIMED_OUT.to_owned());
                        Fetch::Left
                    }
                    error => {
                        self.skipped.push(format!("couldn't be downloaded ({error})"));
                        Fetch::Left
                    }
                }
            }
        }
    }

    /// The song as its file describes it, which the library only learns once the file is here.
    /// `None` when the file can't be read and the song is left out.
    fn read(&mut self, track: &SyncTrack) -> Option<Track> {
        let id = track.library_id;
        let song = {
            let mut library = self.library.queries();
            let _ = library.refresh_cloud_status(&[id]);
            library.track(id).ok().flatten()
        };
        let Some(song) = song.filter(|song| !song.in_cloud) else {
            self.skipped.push(UNREADABLE.to_owned());
            return None;
        };
        // The original, shrunk here and kept only for this sync: the library's own thumbnails are
        // cached for good, and a song this sync gives back would leave its thumbnail for nothing.
        if self.artwork
            && let Some(cover) = self.library.artwork(id, None).and_then(|art| unsub_library::shrink_to_jpeg(&art.bytes, COVER_PIXELS))
        {
            self.covers.keep(id, &cover);
        }
        self.fetched.push(id);
        Some(song)
    }

    fn release_downloaded(&mut self) {
        if let Some(path) = self.downloaded.take() {
            // iCloud declines a song it hasn't taken the changes to yet; it stays, and nothing is lost.
            let _ = self.cloud.release(&path);
        }
    }

    /// Ends the sync's dealings with iCloud and the cache, finished or not: the last song goes
    /// back, so do abandoned downloads that have since completed, and the library learns which
    /// songs came down and which are cloud-only again.
    fn finish(&mut self) -> Fetched {
        self.release_downloaded();
        if let Some(done) = self.converted.take() {
            let _ = std::fs::remove_file(done);
        }
        let abandoned = std::mem::take(&mut self.abandoned);
        let (finished, unfinished): (Vec<PathBuf>, Vec<PathBuf>) = abandoned.into_iter().partition(|path| !self.cloud.is_placeholder(path));
        for path in &finished {
            let _ = self.cloud.release(path);
        }
        // A batch at a time, so the song list is never held up for the whole of a big refresh.
        for ids in self.fetched.chunks(REFRESH_BATCH) {
            let _ = self.library.queries().refresh_cloud_status(ids);
        }
        Fetched { songs: self.fetched.len(), skipped: std::mem::take(&mut self.skipped), unfinished }
    }
}

/// The file to copy for the song: itself when the iPod plays it as it is, else a conversion made
/// in `work_dir`.
fn playable_file(track: &SyncTrack, conversion: Conversion, lossless: bool, work_dir: &Path, cancel: &AtomicBool) -> io::Result<PathBuf> {
    let target = match conversion {
        Conversion::Copy if unsub_playback::ipod_compatible(&track.source_path).unwrap_or(false) => {
            return Ok(track.source_path.clone());
        }
        // The library's details said it would play, but the file itself disagrees.
        Conversion::Copy if lossless => Target::Alac,
        Conversion::Copy => Target::Aac { kbps: FALLBACK_AAC_KBPS },
        Conversion::Alac => Target::Alac,
        Conversion::Aac(kbps) => Target::Aac { kbps },
    };
    let output = work_dir.join(format!("{}.m4a", track.library_id));
    let options =
        TranscodeOptions { target, max_sample_rate: Some(IPOD_MAX_SAMPLE_RATE), max_bit_depth: Some(IPOD_MAX_BIT_DEPTH), keep_tags: true };
    unsub_playback::transcode_file(&track.source_path, &output, &options, &mut |_, _| {}, cancel)
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(output)
}

/// What to tell the user about songs a sync left out: how many, and why.
fn unsynced_notice(reasons: &[String]) -> String {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for reason in reasons {
        *counts.entry(reason).or_default() += 1;
    }
    let why: Vec<String> = counts.iter().map(|(reason, count)| format!("{count} {reason}")).collect();
    let (count, was) = (reasons.len(), if reasons.len() == 1 { "song wasn't" } else { "songs weren't" });
    format!("Synced, but {count} {was} copied: {}", why.join("; "))
}

/// Merges what the iPod recorded (plays, skips, ratings) into the library, as iTunes did on
/// every sync, so the library's counts are what gets written back.
fn merge_play_counts(app: &AppHandle, ipod: &Ipod) -> CommandResult<bool> {
    let deltas = unsub_ipod::read_play_counts_for_sync_back(ipod).map_err(to_error)?;
    if deltas.is_empty() {
        return Ok(false);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let library = app.state::<LibraryState>();
    let mut library = library.queries();
    for (id, delta) in deltas {
        let at = delta.last_played.unwrap_or(now);
        for _ in 0..delta.plays {
            library.record_play(id, at)?;
        }
        for _ in 0..delta.skips {
            library.record_skip(id, at)?;
        }
        if let Some(rating) = delta.rating {
            library.set_rating(id, rating)?;
        }
    }
    Ok(true)
}

pub(super) fn run_ipod_sync(app: &AppHandle, ctx: &Ctx, device_id: &str) -> CommandResult<()> {
    let devices = app.state::<DevicesState>();
    let ipod = fresh_ipod(&devices, device_id)?;
    // Songs playing from the iPod would hold files the sync may remove.
    app.state::<PlaybackState>().stop_if_playing(&PlaySource::Device { device_id: device_id.to_owned() });

    // The library holds what the iPod recorded once this has merged it, so the iPod's record goes
    // at once: left until the end it would be counted again by the next sync after any failure.
    if merge_play_counts(app, &ipod)? {
        unsub_ipod::clear_play_counts(&ipod).map_err(to_error)?;
        let _ = app.emit(library::CHANGED_EVENT, ());
    }
    let settings = app.state::<SyncState>().settings_for(device_id);
    let selection = for_ipod(&select(&app.state::<LibraryState>().queries(), &settings, true)?, settings.convert_kbps);
    let database = unsub_ipod::read_database(&ipod).map_err(to_error)?;
    let plan = unsub_ipod::plan_sync(&ipod, &database, &selection.tracks, &selection.playlists).map_err(to_error)?;

    let leftovers = leftovers_of(app)?;
    leftovers.release(&ICloud, &giving_back(app), crate::playback::now_unix());
    let work_dir = fresh_work_dir(app)?;
    let library = app.state::<LibraryState>();
    let covers = Covers::new(work_dir.join("covers"));
    let position = Cell::new((0, 0));
    let emit = |progress: SyncProgress| {
        let status = SyncStatus { device_id: device_id.to_owned(), progress };
        // Each song is its own stage: one report is made per song, before its slow copy, and must show.
        let stage = (discriminant(&status.progress.stage), status.progress.done);
        ctx.progress(status.progress.done, Some(status.progress.total), stage, status);
    };
    let mut songs = IpodSongs {
        library: &library,
        cloud: &ICloud,
        covers: &covers,
        emit: &emit,
        position: &position,
        conversions: &selection.conversions,
        in_cloud: &selection.in_cloud,
        convert_kbps: settings.convert_kbps,
        work_dir: &work_dir,
        cancel: ctx.cancel_flag(),
        release: settings.remove_downloaded,
        artwork: unsub_ipod::artwork_supported(&ipod),
        stall: DOWNLOAD_STALL,
        timeouts: 0,
        converted: None,
        downloaded: None,
        abandoned: Vec::new(),
        fetched: Vec::new(),
        skipped: Vec::new(),
    };
    let mut prepare = |track: &SyncTrack| Ok(songs.prepare(track));
    let mut progress = |progress: SyncProgress| {
        if progress.stage == SyncStage::Preparing {
            position.set((progress.done, progress.total));
        }
        emit(progress);
    };
    // A downloaded song's cover was read while it was here; the rest are read from the library now.
    let mut cover = |library_id: i64| covers.get(library_id).or_else(|| library.artwork(library_id, None).map(|art| art.bytes));
    let result = unsub_ipod::apply_sync(&ipod, &plan, &selection.tracks, &selection.playlists, &mut prepare, &mut cover, &mut progress);
    let fetched = songs.finish();
    leftovers.add(&fetched.unfinished, device_id, crate::playback::now_unix());
    covers.clear();
    if fetched.songs > 0 {
        let _ = app.emit(library::CHANGED_EVENT, ());
    }

    // Whatever became of the sync, songs may have been committed, and the view of the iPod is of
    // what it held before.
    let reconnected = devices.reconnect(&ipod.mount_path);
    let _ = app.emit(devices::CHANGED_EVENT, ());
    result.map_err(to_error)?;
    reconnected.map_err(to_error)?;
    // A stop commits what was copied, so it ends like a sync and the notice is the one place to say so.
    let message = if ctx.cancelled() {
        // Songs are removed only at the end of a sync, unless there was no room without.
        let removed = if plan.removals_first && !plan.remove.is_empty() {
            let count = plan.remove.len();
            format!(" {count} {} removed to make room.", if count == 1 { "song was" } else { "songs were" })
        } else {
            String::new()
        };
        Some(format!("Sync stopped. The songs already copied are on the iPod.{removed}"))
    } else if fetched.skipped.is_empty() {
        None
    } else {
        Some(unsynced_notice(&fetched.skipped))
    };
    if let Some(message) = message {
        ctx.notice(message);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::super::cloud::fake::FakeCloud;
    use super::*;
    use crate::test_support::track;

    #[test]
    fn files_the_ipod_plays_are_copied_and_the_rest_converted() {
        let flac = track(1, "FLAC", MediaKind::Music);
        let hires_alac = Track { bit_depth: Some(24), sample_rate: Some(96_000), ..track(2, "ALAC", MediaKind::Music) };
        let surround = Track { channels: Some(6), ..track(3, "WAV", MediaKind::Music) };
        assert!(Conversion::for_track(&flac, None) == Conversion::Alac);
        assert!(Conversion::for_track(&hires_alac, None) == Conversion::Alac);
        assert!(Conversion::for_track(&surround, None) == Conversion::Alac);
        assert!(Conversion::for_track(&track(4, "ALAC", MediaKind::Music), None) == Conversion::Copy);
        assert!(Conversion::for_track(&track(5, "MP3", MediaKind::Music), None) == Conversion::Copy);
        assert!(Conversion::for_track(&track(6, "Opus", MediaKind::Music), None) == Conversion::Aac(FALLBACK_AAC_KBPS));
    }

    #[test]
    fn converting_to_aac_takes_lossless_and_higher_bit_rates_only() {
        let mp3_192 = Track { bitrate_kbps: Some(192), ..track(1, "MP3", MediaKind::Music) };
        assert!(Conversion::for_track(&track(2, "MP3", MediaKind::Music), Some(256)) == Conversion::Aac(256));
        assert!(Conversion::for_track(&mp3_192, Some(256)) == Conversion::Copy);
        assert!(Conversion::for_track(&track(3, "FLAC", MediaKind::Music), Some(128)) == Conversion::Aac(128));
    }

    #[test]
    fn converted_rates_stay_in_their_family() {
        assert_eq!(ipod_rate(44_100), 44_100);
        assert_eq!(ipod_rate(48_000), 48_000);
        assert_eq!(ipod_rate(88_200), 44_100);
        assert_eq!(ipod_rate(176_400), 44_100);
        assert_eq!(ipod_rate(96_000), 48_000);
        assert_eq!(ipod_rate(192_000), 48_000);
    }

    #[test]
    fn converted_songs_describe_the_file_the_ipod_gets() {
        let hires = Track { sample_rate: Some(96_000), bit_depth: Some(24), ..track(1, "FLAC", MediaKind::Music) };
        let alac = sync_track(&hires, Conversion::Alac);
        assert_eq!((alac.bitrate_kbps, alac.sample_rate), (None, Some(48_000)));
        let aac = sync_track(&hires, Conversion::Aac(256));
        assert_eq!((aac.bitrate_kbps, aac.sample_rate), (Some(256), Some(48_000)));
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/playback/tests/fixtures").join(name)
    }

    /// 1x1 PNGs, which the cover code can decode and shrink into a JPEG.
    const RED: [u8; 69] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63,
        0x38, 0xc1, 0xc0, 0x00, 0x00, 0x02, 0x5c, 0x00, 0xc9, 0x43, 0xc8, 0x12, 0x46, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
        0x42, 0x60, 0x82,
    ];
    const BLUE: [u8; 69] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63,
        0x60, 0x60, 0x38, 0x01, 0x00, 0x00, 0xcc, 0x00, 0xc9, 0x27, 0x72, 0xc1, 0x5a, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
        0x42, 0x60, 0x82,
    ];

    /// The fixture MP3 with `picture` as its front cover, written to `path`.
    fn mp3_with_cover(path: &Path, picture: &[u8]) {
        let fixture = std::fs::read(fixture("sine.mp3")).unwrap();
        // Its own ID3v2 tag is a 10-byte header and 0x23 bytes of frames.
        let audio = &fixture[10 + 0x23..];
        let mut apic = vec![0];
        apic.extend_from_slice(b"image/png\0");
        apic.extend_from_slice(&[3, 0]);
        apic.extend_from_slice(picture);
        let mut frame = b"APIC".to_vec();
        frame.extend_from_slice(&(apic.len() as u32).to_be_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&apic);
        let size = frame.len() as u32;
        let synchsafe = [(size >> 21) as u8 & 0x7f, (size >> 14) as u8 & 0x7f, (size >> 7) as u8 & 0x7f, size as u8 & 0x7f];
        let tag = [b"ID3\x03\x00\x00".as_slice(), &synchsafe, &frame].concat();
        std::fs::write(path, [tag.as_slice(), audio].concat()).unwrap();
    }

    /// What the library knew of a song in iCloud before it was ever downloaded.
    fn placeholder(id: i64, path: &Path) -> Track {
        Track {
            path: path.to_string_lossy().into(),
            title: format!("{id:02} From Its Name"),
            in_cloud: true,
            duration_ms: 0,
            bitrate_kbps: None,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            ..track(id, "MP3", MediaKind::Music)
        }
    }

    /// A library, a cloud and the folders a sync works in, for the tests of `IpodSongs`.
    struct Bench {
        dir: tempfile::TempDir,
        library: LibraryState,
        covers: Covers,
        position: Cell<(u64, u64)>,
        events: RefCell<Vec<SyncProgress>>,
        conversions: HashMap<i64, (Conversion, bool)>,
        in_cloud: HashSet<i64>,
        cancel: AtomicBool,
    }

    impl Bench {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let library = LibraryState::open(dir.path().join("library.db"), dir.path().join("art")).unwrap();
            let covers = Covers::new(dir.path().join("covers"));
            Self {
                dir,
                library,
                covers,
                position: Cell::new((0, 0)),
                events: RefCell::default(),
                conversions: HashMap::new(),
                in_cloud: HashSet::new(),
                cancel: AtomicBool::new(false),
            }
        }

        /// A song in the library whose file is the fixture MP3 (with a cover, if given), down in
        /// the library as only in iCloud if `in_cloud`.
        fn song(&mut self, id: i64, in_cloud: bool, cover: Option<&[u8]>) -> SyncTrack {
            let path = self.dir.path().join(format!("{id}.mp3"));
            match cover {
                Some(picture) => mp3_with_cover(&path, picture),
                None => std::fs::copy(fixture("sine.mp3"), &path).map(drop).unwrap(),
            }
            let row = if in_cloud {
                placeholder(id, &path)
            } else {
                Track { path: path.to_string_lossy().into(), ..track(id, "MP3", MediaKind::Music) }
            };
            self.library.queries().import_tracks(std::slice::from_ref(&row)).unwrap();
            if in_cloud {
                self.in_cloud.insert(id);
            }
            sync_track(&row, Conversion::Copy)
        }

        fn songs<'a>(&'a self, cloud: &'a dyn Cloud, emit: &'a dyn Fn(SyncProgress), release: bool) -> IpodSongs<'a> {
            IpodSongs {
                library: &self.library,
                cloud,
                covers: &self.covers,
                emit,
                position: &self.position,
                conversions: &self.conversions,
                in_cloud: &self.in_cloud,
                convert_kbps: None,
                work_dir: self.dir.path(),
                cancel: &self.cancel,
                release,
                artwork: true,
                stall: Duration::from_secs(60),
                timeouts: 0,
                converted: None,
                downloaded: None,
                abandoned: Vec::new(),
                fetched: Vec::new(),
                skipped: Vec::new(),
            }
        }
    }

    fn cloud_with<'a>(songs: &[&SyncTrack]) -> FakeCloud<'a> {
        FakeCloud { placeholders: RefCell::new(songs.iter().map(|song| song.source_path.clone()).collect()), ..FakeCloud::default() }
    }

    #[test]
    fn a_song_that_is_here_is_copied_as_it_is_and_never_given_back() {
        let mut bench = Bench::new();
        let local = bench.song(1, false, None);
        let cloud = cloud_with(&[]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        let prepared = songs.prepare(&local);
        let fetched = songs.finish();

        assert_eq!(prepared, Prepared::File { path: local.source_path.clone(), track: None });
        assert!(cloud.downloads.borrow().is_empty() && cloud.released.borrow().is_empty());
        assert!(bench.events.borrow().is_empty());
        assert!(fetched.songs == 0 && fetched.skipped.is_empty() && fetched.unfinished.is_empty());
    }

    #[test]
    fn a_song_only_in_icloud_is_downloaded_described_by_its_file_and_given_back_once_copied() {
        let mut bench = Bench::new();
        let cloudy = bench.song(1, true, None);
        let local = bench.song(2, false, None);
        let cloud = cloud_with(&[&cloudy]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        bench.position.set((3, 10));

        let Prepared::File { path, track: Some(described) } = songs.prepare(&cloudy) else {
            panic!("expected the song and what its file says of it");
        };

        assert_eq!(path, cloudy.source_path);
        assert_eq!(*cloud.downloads.borrow(), std::slice::from_ref(&cloudy.source_path));
        let [event] = bench.events.borrow().clone().try_into().unwrap();
        assert_eq!(event, SyncProgress { stage: SyncStage::Downloading, done: 3, total: 10, current: "01 From Its Name".into() });
        // The placeholder had no length and a title from its name; the file has both.
        assert!(described.duration_ms > 0, "{}", described.duration_ms);
        assert_eq!(described.library_id, 1);
        let row = bench.library.queries().track(1).unwrap().unwrap();
        assert!(!row.in_cloud && row.duration_ms == described.duration_ms);
        // It is still being copied, so it is not given back yet.
        assert!(cloud.released.borrow().is_empty());

        // Asking for the next song means the last one was copied.
        songs.prepare(&local);
        assert_eq!(*cloud.released.borrow(), std::slice::from_ref(&cloudy.source_path));
        let fetched = songs.finish();
        assert_eq!(fetched.songs, 1);
        assert_eq!(cloud.released.borrow().len(), 1, "a song that was already here is never given back");
    }

    #[test]
    fn a_song_the_library_has_as_in_icloud_but_that_is_here_is_read_and_never_given_back() {
        let mut bench = Bench::new();
        let downloaded_since = bench.song(1, true, None);
        let cloud = cloud_with(&[]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        let Prepared::File { track: Some(described), .. } = songs.prepare(&downloaded_since) else {
            panic!("expected the song as its file describes it");
        };
        songs.finish();

        assert!(described.duration_ms > 0);
        assert!(cloud.downloads.borrow().is_empty() && cloud.released.borrow().is_empty());
        assert!(bench.events.borrow().is_empty(), "nothing was downloaded to announce");
        assert!(!bench.library.queries().track(1).unwrap().unwrap().in_cloud);
    }

    #[test]
    fn the_last_song_is_given_back_when_the_sync_ends_and_none_are_when_the_setting_is_off() {
        for (release, given_back) in [(true, 1), (false, 0)] {
            let mut bench = Bench::new();
            let cloudy = bench.song(1, true, None);
            let cloud = cloud_with(&[&cloudy]);
            let emit = |progress| bench.events.borrow_mut().push(progress);
            let mut songs = bench.songs(&cloud, &emit, release);

            songs.prepare(&cloudy);
            songs.finish();

            assert_eq!(cloud.released.borrow().len(), given_back);
        }
    }

    #[test]
    fn a_download_that_fails_leaves_the_song_out_and_the_sync_goes_on() {
        let mut bench = Bench::new();
        let broken = bench.song(1, true, None);
        let fine = bench.song(2, true, None);
        let mut cloud = cloud_with(&[&broken, &fine]);
        cloud.failing.insert(broken.source_path.clone());
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        assert_eq!(songs.prepare(&broken), Prepared::Skip);
        assert!(matches!(songs.prepare(&fine), Prepared::File { track: Some(_), .. }));
        let fetched = songs.finish();

        assert_eq!(fetched.skipped, ["couldn't be downloaded (i/o error: offline)"]);
        assert!(fetched.unfinished.is_empty(), "a download that failed has no request left to answer");
        assert_eq!(fetched.songs, 1);
        assert!(bench.library.queries().track(1).unwrap().unwrap().in_cloud, "the library still says it is in iCloud");
    }

    #[test]
    fn three_downloads_in_a_row_that_stall_mean_the_rest_are_not_tried() {
        let mut bench = Bench::new();
        let stuck: Vec<SyncTrack> = (1..=4).map(|id| bench.song(id, true, None)).collect();
        let after = bench.song(5, false, None);
        let cloud = FakeCloud { slow: Some(Duration::MAX), ..cloud_with(&stuck.iter().collect::<Vec<_>>()) };
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        songs.stall = Duration::from_millis(20);

        for song in &stuck {
            assert_eq!(songs.prepare(song), Prepared::Skip);
        }
        // A song that is already here needs no iCloud.
        assert!(matches!(songs.prepare(&after), Prepared::File { .. }));

        assert_eq!(cloud.downloads.borrow().len(), 3, "the fourth was not even asked for");
        assert_eq!(songs.finish().skipped, [TIMED_OUT, TIMED_OUT, TIMED_OUT, NO_RESPONSE]);
    }

    #[test]
    fn a_download_that_gets_through_starts_the_count_of_stalls_again() {
        let mut bench = Bench::new();
        let songs_in_cloud: Vec<SyncTrack> = (1..=6).map(|id| bench.song(id, true, None)).collect();
        let mut cloud = cloud_with(&songs_in_cloud.iter().collect::<Vec<_>>());
        // Two stall, one comes through, two stall: five tries in all, never three in a row.
        for index in [0, 1, 3, 4] {
            cloud.hanging.insert(songs_in_cloud[index].source_path.clone());
        }
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        songs.stall = Duration::from_millis(20);

        let outcomes: Vec<bool> = songs_in_cloud[..5].iter().map(|song| matches!(songs.prepare(song), Prepared::File { .. })).collect();

        assert_eq!(outcomes, [false, false, true, false, false]);
        assert_eq!(cloud.downloads.borrow().len(), 5);
        assert!(!songs.finish().skipped.contains(&NO_RESPONSE.to_owned()));
    }

    #[test]
    fn a_big_file_is_given_longer_before_it_counts_as_stalled() {
        assert_eq!(stall_limit(Duration::from_secs(180), 10 << 20), Duration::from_secs(180));
        assert_eq!(stall_limit(Duration::from_secs(180), 1 << 30), Duration::from_secs(4096));

        let mut bench = Bench::new();
        let book = bench.song(1, true, None);
        // Nothing arrives for far longer than the floor, but the file is a gigabyte.
        let cloud = FakeCloud { slow: Some(Duration::from_millis(80)), total: Some(1 << 40), ..cloud_with(&[&book]) };
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        songs.stall = Duration::from_millis(20);

        assert!(matches!(songs.prepare(&book), Prepared::File { .. }));
        assert!(songs.finish().skipped.is_empty());
    }

    #[test]
    fn stopping_during_a_download_ends_the_sync_and_is_not_counted_as_a_song_left_out() {
        let mut bench = Bench::new();
        let cloudy = bench.song(1, true, None);
        let cloud = FakeCloud { slow: Some(Duration::MAX), cancels: Some(&bench.cancel), ..cloud_with(&[&cloudy]) };
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        let prepared = songs.prepare(&cloudy);
        let fetched = songs.finish();

        assert_eq!(prepared, Prepared::Stop);
        assert!(fetched.skipped.is_empty());
        // iCloud was asked for it and may yet send it: it is noted, to be given back when it does.
        assert_eq!(fetched.unfinished, std::slice::from_ref(&cloudy.source_path));
        assert!(cloud.released.borrow().is_empty(), "still a placeholder, so nothing to give back yet");
    }

    #[test]
    fn a_sync_that_is_stopped_before_a_song_is_asked_for_stops() {
        let mut bench = Bench::new();
        let local = bench.song(1, false, None);
        bench.cancel.store(true, Ordering::Relaxed);
        let cloud = cloud_with(&[]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        assert_eq!(songs.prepare(&local), Prepared::Stop);
    }

    #[test]
    fn a_download_given_up_on_that_finishes_later_is_given_back_when_the_sync_ends() {
        let mut bench = Bench::new();
        let (late, never) = (bench.song(1, true, None), bench.song(2, true, None));
        let cloud = FakeCloud { slow: Some(Duration::MAX), ..cloud_with(&[&late, &never]) };
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        songs.stall = Duration::from_millis(20);
        assert_eq!(songs.prepare(&late), Prepared::Skip);
        // The offline Mac gets its connection back, and iCloud answers the request.
        cloud.placeholders.borrow_mut().remove(&late.source_path);
        cloud.placeholders.borrow_mut().insert(never.source_path.clone());
        songs.abandoned.push(never.source_path.clone());

        let fetched = songs.finish();

        assert_eq!(*cloud.released.borrow(), std::slice::from_ref(&late.source_path));
        assert_eq!(fetched.unfinished, std::slice::from_ref(&never.source_path));
    }

    #[test]
    fn with_the_setting_off_an_abandoned_download_is_nobodys_to_give_back() {
        let mut bench = Bench::new();
        let stuck = bench.song(1, true, None);
        let cloud = FakeCloud { slow: Some(Duration::MAX), ..cloud_with(&[&stuck]) };
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, false);
        songs.stall = Duration::from_millis(20);

        songs.prepare(&stuck);

        assert!(songs.finish().unfinished.is_empty());
    }

    #[test]
    fn a_download_that_cannot_be_read_is_left_out() {
        let mut bench = Bench::new();
        let broken = bench.song(1, true, None);
        std::fs::write(&broken.source_path, b"this is not audio").unwrap();
        let cloud = cloud_with(&[&broken]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        assert_eq!(songs.prepare(&broken), Prepared::Skip);

        assert_eq!(songs.finish().skipped, [UNREADABLE]);
    }

    #[test]
    fn a_song_that_will_not_convert_is_left_out_and_the_sync_goes_on() {
        let mut bench = Bench::new();
        let song = bench.song(1, false, None);
        std::fs::write(&song.source_path, b"this is not audio").unwrap();
        bench.conversions.insert(1, (Conversion::Alac, true));
        let cloud = cloud_with(&[]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        assert_eq!(songs.prepare(&song), Prepared::Skip);
        assert_eq!(songs.finish().skipped, [NOT_CONVERTED]);
    }

    #[test]
    fn a_downloaded_songs_cover_is_kept_as_a_jpeg_from_the_moment_it_is_here() {
        let mut bench = Bench::new();
        let (one, two, three) = (bench.song(1, true, Some(&RED)), bench.song(2, true, Some(&RED)), bench.song(3, true, Some(&BLUE)));
        let cloud = cloud_with(&[&one, &two, &three]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        for song in [&one, &two, &three] {
            songs.prepare(song);
        }
        songs.finish();

        let kept = |id| bench.covers.get(id).unwrap();
        assert!(kept(1).starts_with(&[0xff, 0xd8]), "shrunk into a JPEG, not the picture as it was embedded");
        assert_eq!(kept(1), kept(2));
        assert_ne!(kept(1), kept(3));
        assert_eq!(bench.covers.get(4), None);
        assert_eq!(std::fs::read_dir(bench.dir.path().join("covers")).unwrap().count(), 2, "one file per distinct picture");
        bench.covers.clear();
        assert_eq!(bench.covers.get(1), None);
        let cached = std::fs::read_dir(bench.dir.path().join("art")).map_or(0, Iterator::count);
        assert_eq!(cached, 0, "no thumbnail is left in the library's cache for a song that goes back to iCloud");
    }

    #[test]
    fn no_cover_is_kept_for_an_ipod_that_shows_no_album_art() {
        let mut bench = Bench::new();
        let song = bench.song(1, true, Some(&RED));
        let cloud = cloud_with(&[&song]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        songs.artwork = false;

        assert!(matches!(songs.prepare(&song), Prepared::File { .. }));

        assert_eq!(bench.covers.get(1), None);
        assert!(!bench.dir.path().join("covers").exists());
    }

    #[test]
    fn songs_left_out_are_reported_by_how_many_and_why() {
        let reasons: Vec<String> = [TIMED_OUT, UNREADABLE, TIMED_OUT, NO_RESPONSE].map(String::from).into();
        assert_eq!(
            unsynced_notice(&reasons),
            "Synced, but 4 songs weren't copied: 1 couldn't be read once downloaded; 1 skipped because iCloud isn't responding; 2 timed out waiting for iCloud"
        );
        assert_eq!(unsynced_notice(&reasons[..1]), "Synced, but 1 song wasn't copied: 1 timed out waiting for iCloud");
    }
}
