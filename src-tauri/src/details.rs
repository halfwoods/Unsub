//! Advanced › Get Song Details from iCloud: reads the real tags and covers of the songs that are only in iCloud, one at a
//! time in the background. The reading is the library's; this is what runs around it, a job that gives way to playback,
//! can be paused, and carries on after a restart.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use unsub_library::{Blocked, DetailsHost, DetailsProgress, DetailsSummary};

use crate::error::CommandResult;
use crate::jobs::{self, Ctx, Job, Jobs, Kind};
use crate::library::{CHANGED_EVENT, LibraryState};
use crate::playback::{PlaybackState, now_unix};
use crate::sync::{DETAILS_DEVICE, Leftovers};

/// Progress while song details are being read, and the `null` that ends it.
pub const DETAILS_EVENT: &str = "library://details";

/// How long a paused read, or one that gives way to playback, waits between looks at whether it may go on.
const HOLD_POLL: Duration = Duration::from_millis(500);
/// A read that was going when the app quit starts again this long after launch, when the app has done its own start-up.
const LAUNCH_SETTLE: Duration = Duration::from_secs(20);
/// Disk space is shown in the decimal gigabytes the disk's maker and Finder count in.
const GB: f64 = 1e9;
/// Where iCloud keeps its files, relative to the home folder. The read works only there on a Mac: `brctl` can't give
/// back what Dropbox, Google Drive or OneDrive have put in a placeholder's place, so those songs would only pile up.
const ICLOUD_FILES: &str = "Library/Mobile Documents";
/// iCloud Drive's own folder below it, which goes when iCloud Drive is turned off.
const ICLOUD_DRIVE: &str = "com~apple~CloudDocs";

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Running,
    Paused,
    /// Paused by itself, because the disk is too full to download onto.
    LowSpace,
    /// Paused by itself, because downloads in a row have brought in nothing.
    NoResponse,
    /// Paused by itself, because songs in a row have no file: iCloud Drive is off, or the folder has moved.
    FilesMissing,
    /// Paused by itself, because the system keeps leaving downloaded songs on the disk.
    NotGivenBack,
}

/// What the window is told while the read goes on.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    read: u64,
    skipped: u64,
    missing: u64,
    total: u64,
    title: String,
    phase: Phase,
}

#[derive(Default)]
pub struct DetailsState {
    paused: AtomicBool,
    /// The latest progress, so a pause can be shown at once instead of when the song in hand is done.
    last: Mutex<Option<Progress>>,
}

impl DetailsState {
    fn last(&self) -> MutexGuard<'_, Option<Progress>> {
        self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The latest progress, as it is in `phase`.
    fn in_phase(&self, phase: Phase) -> Option<Progress> {
        let mut last = self.last();
        let progress = last.as_mut()?;
        progress.phase = phase;
        Some(progress.clone())
    }
}

/// What the read asks of playback, so that a test can answer in its place.
trait Listening {
    fn fetching(&self) -> bool;
    fn in_use(&self) -> Vec<i64>;
}

impl Listening for AppHandle {
    fn fetching(&self) -> bool {
        self.state::<PlaybackState>().fetching()
    }

    fn in_use(&self) -> Vec<i64> {
        self.state::<PlaybackState>().in_use()
    }
}

struct Host<'a, P: Listening> {
    ctx: &'a Ctx,
    state: &'a DetailsState,
    library: &'a LibraryState,
    playback: &'a P,
    /// The downloads asked of iCloud and not given back, which survive a quit, and which a sync's own list shares.
    leftovers: &'a Leftovers,
    hold_poll: Duration,
}

impl<P: Listening> Host<'_, P> {
    /// Waits for as long as the user has paused the read, or playback is downloading a song of its own. False if it was
    /// stopped meanwhile.
    fn hold(&self) -> bool {
        loop {
            if self.ctx.cancelled() {
                return false;
            }
            if !self.state.paused.load(Ordering::Relaxed) && !self.playback.fetching() {
                return true;
            }
            thread::sleep(self.hold_poll);
        }
    }
}

impl<P: Listening> DetailsHost for Host<'_, P> {
    fn cancel_flag(&self) -> &AtomicBool {
        self.ctx.cancel_flag()
    }

    fn wait_turn(&self) -> bool {
        self.hold()
    }

    /// An unreadable volume counts as full: better to wait than to fill a disk that can't be seen.
    fn free_bytes(&self, path: &Path) -> u64 {
        fs4::available_space(path).unwrap_or(0)
    }

    fn in_use(&self) -> Vec<i64> {
        self.playback.in_use()
    }

    /// Pauses as the user's Pause does, so Resume is what tries again, and a restart doesn't go straight back to a full disk
    /// or an iCloud that isn't answering.
    fn blocked(&self, reason: Blocked) -> bool {
        let (phase, notice) = match reason {
            // Finder's figure includes space macOS will make by itself, so it can be bigger than this.
            Blocked::LowSpace { free, needed } => (
                Phase::LowSpace,
                format!(
                    "Getting song details is paused: only {:.1} GB is free on the disk (Finder may show more, as it counts space the Mac can clear by itself), and the next song needs {:.1} GB. Free up some space, then choose Resume.",
                    free as f64 / GB,
                    needed as f64 / GB
                ),
            ),
            Blocked::NoResponse => (
                Phase::NoResponse,
                "Getting song details is paused: iCloud isn't responding. Check your connection, then choose Resume.".to_owned(),
            ),
            Blocked::FilesMissing => (
                Phase::FilesMissing,
                "Getting song details is paused: the songs' files can't be found. Check that iCloud Drive is on and the folder hasn't moved, then choose Resume."
                    .to_owned(),
            ),
            Blocked::NotGivenBack => (
                Phase::NotGivenBack,
                "Getting song details is paused: iCloud isn't taking the downloaded songs back, and they would fill the disk. Choose Resume to try again."
                    .to_owned(),
            ),
        };
        self.state.paused.store(true, Ordering::Relaxed);
        let _ = self.library.queries().set_cloud_details_wanted(false);
        self.ctx.notice(notice);
        if let Some(progress) = self.state.in_phase(phase) {
            self.ctx.emit(DETAILS_EVENT, progress);
        }
        self.hold()
    }

    fn progress(&self, progress: &DetailsProgress) {
        let status = Progress {
            read: progress.read,
            skipped: progress.skipped,
            missing: progress.missing,
            total: progress.total,
            title: progress.title.clone(),
            phase: Phase::Running,
        };
        *self.state.last() = Some(status.clone());
        let done = progress.read + progress.skipped + progress.missing;
        // Songs passed over come thousands to the second, so those reports are held to the job runner's pace; one for a
        // download, which takes seconds, is always shown, or the window would name the song before it for the whole wait.
        self.ctx.progress(done, Some(progress.total), progress.download.then_some(done), status);
    }

    fn changed(&self) {
        self.ctx.emit(CHANGED_EVENT, ());
    }

    fn fetching(&self, path: &Path) {
        self.leftovers.add(&[path.to_path_buf()], DETAILS_DEVICE, now_unix());
    }

    fn fetched_earlier(&self, path: &Path) -> bool {
        self.leftovers.contains(path, DETAILS_DEVICE, now_unix())
    }

    fn given_back(&self, path: &Path) {
        self.leftovers.forget(path, now_unix());
    }
}

/// What a finished or stopped read tells the user.
fn finished_notice(result: &Result<DetailsSummary, impl std::fmt::Display>, stopped: bool) -> String {
    let songs = |count: u64| format!("{count} {}", if count == 1 { "song" } else { "songs" });
    match result {
        Err(error) => format!("Couldn't get song details: {error}"),
        Ok(summary) if summary.read + summary.skipped + summary.missing == 0 && !stopped => "No songs need their details read".to_owned(),
        Ok(summary) => {
            let mut got = format!("details for {}", songs(summary.read));
            if summary.skipped > 0 {
                got += &format!("; {} couldn't be downloaded or read", songs(summary.skipped));
            }
            if summary.missing > 0 {
                got += &format!("; {} couldn't be found", songs(summary.missing));
            }
            if stopped { format!("Stopped after getting {got}") } else { format!("Got {got}") }
        }
    }
}

/// Where the read works: on a Mac, only iCloud Drive's own folder; elsewhere anywhere, since the platform's own cloud
/// provider is the only one it can tell to give a file back.
fn within(app: &AppHandle) -> CommandResult<PathBuf> {
    if cfg!(target_os = "macos") {
        // Ends in a separator, so that a folder merely named like it isn't taken for it.
        Ok(app.path().home_dir().map_err(|error| error.to_string())?.join(ICLOUD_FILES).join(""))
    } else {
        Ok(PathBuf::new())
    }
}

/// The folder the songs live under, whose libraries (the first folders below it) are gone when iCloud Drive is off or one
/// is renamed, rather than the songs having been deleted. Where there is none to name, nothing is blamed on missing files.
fn root(app: &AppHandle) -> CommandResult<Option<PathBuf>> {
    if cfg!(target_os = "macos") {
        Ok(Some(app.path().home_dir().map_err(|error| error.to_string())?.join(ICLOUD_FILES).join(ICLOUD_DRIVE)))
    } else {
        Ok(None)
    }
}

fn read(app: &AppHandle, ctx: &Ctx) -> CommandResult<DetailsSummary> {
    let state = app.state::<DetailsState>();
    let library = app.state::<LibraryState>();
    let leftovers = crate::sync::leftovers_of(app)?;
    let host = Host { ctx, state: &state, library: &library, playback: app, leftovers: &leftovers, hold_poll: HOLD_POLL };
    Ok(library.job_connection()?.read_cloud_details(&within(app)?, root(app)?.as_deref(), &host)?)
}

fn run(app: &AppHandle, ctx: &Ctx) {
    let state = app.state::<DetailsState>();
    let library = app.state::<LibraryState>();
    state.paused.store(false, Ordering::Relaxed);
    let _ = library.queries().set_cloud_details_wanted(true);
    let result = read(app, ctx);
    *state.last() = None;
    // Over for good, whether it finished or was stopped; only a quit leaves the wish behind.
    let _ = library.queries().set_cloud_details_wanted(false);
    // Downloads that landed after the read stopped waiting for them (or a song it held for playback) go back too.
    crate::sync::give_back_leftovers(app);
    ctx.notice(finished_notice(&result, ctx.cancelled()));
}

fn start(app: &AppHandle) -> CommandResult<()> {
    jobs::start(app, Job::new(Kind::Details, DETAILS_EVENT), run)
}

/// Starts the read again after a quit that interrupted it. Not at once: the library, the devices and the window are all
/// starting too. Not at all if the songs' library folder isn't there (iCloud Drive off, or the folder renamed): the wish
/// is kept, and the next launch looks again.
pub fn resume_on_launch(app: &AppHandle) {
    if !app.state::<LibraryState>().queries().cloud_details_wanted().unwrap_or(false) {
        return;
    }
    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(LAUNCH_SETTLE);
        // On a connection of its own, since looking for a folder on a drive that has gone can take a while.
        let there = (|| {
            let (within, root) = (within(&app).ok()?, root(&app).ok()?);
            app.state::<LibraryState>().job_connection().ok()?.unread_cloud_library_there(&within, root.as_deref()).ok()
        })();
        if there == Some(true) {
            // Refused if the user has started it by hand meanwhile, which is as it should be.
            let _ = start(&app);
        }
    });
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailsStatus {
    /// How many songs are waiting to be read, and what downloading their files comes to.
    songs: u64,
    bytes: u64,
    running: bool,
    paused: bool,
}

#[tauri::command]
pub fn details_status(
    app: AppHandle,
    library: State<'_, LibraryState>,
    jobs: State<'_, Jobs>,
    state: State<'_, DetailsState>,
) -> CommandResult<DetailsStatus> {
    let totals = library.queries().unread_cloud_totals(&within(&app)?)?;
    let running = jobs.is_running(Kind::Details);
    Ok(DetailsStatus { songs: totals.songs, bytes: totals.bytes, running, paused: running && state.paused.load(Ordering::Relaxed) })
}

#[tauri::command]
pub fn details_start(app: AppHandle) -> CommandResult<()> {
    start(&app)
}

/// Pauses the read after the song in hand, or lets it go on. The window is told at once.
#[tauri::command]
pub fn details_set_paused(app: AppHandle, paused: bool) {
    if !app.state::<Jobs>().is_running(Kind::Details) {
        return;
    }
    let state = app.state::<DetailsState>();
    state.paused.store(paused, Ordering::Relaxed);
    let _ = app.state::<LibraryState>().queries().set_cloud_details_wanted(!paused);
    if let Some(progress) = state.in_phase(if paused { Phase::Paused } else { Phase::Running }) {
        let _ = app.emit(DETAILS_EVENT, progress);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::*;
    use crate::jobs::Events;

    #[derive(Default)]
    struct Recording(Mutex<Vec<(String, Value)>>);

    impl Events for Recording {
        fn emit(&self, event: &str, payload: Value) {
            self.0.lock().unwrap().push((event.to_owned(), payload));
        }
    }

    impl Recording {
        fn on(&self, event: &str) -> Vec<Value> {
            self.0.lock().unwrap().iter().filter(|(name, _)| name == event).map(|(_, payload)| payload.clone()).collect()
        }
    }

    /// Playback that is downloading a song for as many looks as it is given.
    struct Busy {
        looks_left: Cell<u32>,
        in_use: Vec<i64>,
    }

    impl Listening for Busy {
        fn fetching(&self) -> bool {
            let left = self.looks_left.get();
            self.looks_left.set(left.saturating_sub(1));
            left > 0
        }

        fn in_use(&self) -> Vec<i64> {
            self.in_use.clone()
        }
    }

    fn quiet() -> Busy {
        Busy { looks_left: Cell::new(0), in_use: Vec::new() }
    }

    /// Runs `check` as a read would, on the job thread, with the library and the list of downloads in a folder of its own.
    fn within_a_job<P: Listening + Send + 'static>(
        playback: P,
        state: Arc<DetailsState>,
        check: impl FnOnce(&Host<'_, P>) + Send + 'static,
    ) -> Arc<Recording> {
        let dir = tempfile::tempdir().unwrap();
        let library = LibraryState::open(dir.path().join("library.db"), dir.path().join("art")).unwrap();
        let leftovers = Leftovers::new(dir.path().join("leftovers.json"));
        let recording = Arc::new(Recording::default());
        Jobs::default()
            .start(Arc::clone(&recording) as Arc<dyn Events>, Job::new(Kind::Details, DETAILS_EVENT), move |ctx| {
                let host = Host {
                    ctx,
                    state: &state,
                    library: &library,
                    playback: &playback,
                    leftovers: &leftovers,
                    hold_poll: Duration::from_millis(1),
                };
                check(&host);
            })
            .unwrap()
            .join()
            .unwrap();
        recording
    }

    fn progress(read: u64) -> DetailsProgress {
        DetailsProgress { read, skipped: 1, missing: 0, total: 10, title: "Halo".to_owned(), download: false }
    }

    #[test]
    fn reports_come_no_faster_than_the_job_runner_allows_but_the_latest_is_always_kept() {
        let state = Arc::new(DetailsState::default());
        let seen = Arc::clone(&state);
        let recording = within_a_job(quiet(), state, |host| {
            // Songs passed over at once, as when they are already here.
            host.progress(&progress(4));
            host.progress(&progress(5));
            host.progress(&progress(6));
            thread::sleep(Duration::from_millis(120));
            host.progress(&progress(7));
        });

        let reports = recording.on(DETAILS_EVENT);
        assert_eq!(reports.len(), 3, "the first, the first after a pause, and the job's end");
        assert_eq!(
            (reports[0]["read"].as_u64(), reports[0]["total"].as_u64(), reports[0]["phase"].as_str()),
            (Some(4), Some(10), Some("running"))
        );
        assert_eq!(reports[1]["read"], 7);
        assert_eq!(reports[2], Value::Null);
        assert_eq!(seen.last().as_ref().map(|p| p.read), Some(7));
    }

    #[test]
    fn a_download_is_always_announced_so_the_window_names_the_song_it_waits_for() {
        let recording = within_a_job(quiet(), Arc::new(DetailsState::default()), |host| {
            host.progress(&progress(4));
            // Straight after, as when the song before was passed over.
            host.progress(&DetailsProgress { download: true, title: "Waiting for this".to_owned(), ..progress(5) });
            host.progress(&DetailsProgress { download: true, title: "And then this".to_owned(), ..progress(6) });
        });

        let titles: Vec<Value> = recording.on(DETAILS_EVENT).iter().map(|report| report["title"].clone()).collect();
        assert_eq!(titles, [json!("Halo"), json!("Waiting for this"), json!("And then this"), Value::Null]);
    }

    #[test]
    fn the_read_holds_off_while_playback_downloads_a_song_and_goes_on_when_it_is_done() {
        let playback = Busy { looks_left: Cell::new(5), in_use: Vec::new() };
        let turn = Arc::new(Mutex::new(None));
        let found = Arc::clone(&turn);

        within_a_job(playback, Arc::new(DetailsState::default()), move |host| {
            *found.lock().unwrap() = Some(host.wait_turn());
            assert_eq!(host.playback.looks_left.get(), 0, "it looked until playback was done");
        });

        assert_eq!(*turn.lock().unwrap(), Some(true));
    }

    #[test]
    fn a_paused_read_waits_until_it_is_resumed_or_stopped() {
        let state = Arc::new(DetailsState::default());
        state.paused.store(true, Ordering::Relaxed);
        let lifted = Arc::clone(&state);

        within_a_job(quiet(), state, move |host| {
            let resume = thread::spawn({
                let state = lifted;
                move || {
                    thread::sleep(Duration::from_millis(20));
                    state.paused.store(false, Ordering::Relaxed);
                }
            });
            assert!(host.wait_turn());
            resume.join().unwrap();

            host.state.paused.store(true, Ordering::Relaxed);
            host.ctx.cancel_flag().store(true, Ordering::Relaxed);
            assert!(!host.wait_turn(), "a stop ends the wait");
        });
    }

    /// A read that is blocked for `reason`, and resumed by the user once it has said so.
    fn blocked_for(reason: Blocked) -> Arc<Recording> {
        let state = Arc::new(DetailsState::default());
        let lifted = Arc::clone(&state);
        *state.last() = Some(Progress { read: 2, skipped: 0, missing: 0, total: 9, title: "Halo".to_owned(), phase: Phase::Running });
        within_a_job(quiet(), state, move |host| {
            let resume = thread::spawn(move || {
                while !lifted.paused.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(1));
                }
                lifted.paused.store(false, Ordering::Relaxed);
            });
            assert!(host.blocked(reason), "told to go on once resumed");
            resume.join().unwrap();
        })
    }

    #[test]
    fn a_read_blocked_by_a_full_disk_says_how_much_is_free_in_decimal_gigabytes() {
        let recording = blocked_for(Blocked::LowSpace { free: 2_500_000_000, needed: 10_000_000_000 });

        let notices = recording.on(crate::playback::PLAYER_EVENT);
        assert_eq!(
            notices[0]["message"],
            "Getting song details is paused: only 2.5 GB is free on the disk (Finder may show more, as it counts space the Mac can clear by itself), and the next song needs 10.0 GB. Free up some space, then choose Resume."
        );
        assert_eq!(recording.on(DETAILS_EVENT)[0]["phase"], "lowSpace");
    }

    #[test]
    fn each_reason_a_read_is_blocked_for_is_told_apart() {
        for (reason, phase, words) in [
            (Blocked::NoResponse, "noResponse", "iCloud isn't responding"),
            (Blocked::FilesMissing, "filesMissing", "files can't be found"),
            (Blocked::NotGivenBack, "notGivenBack", "isn't taking the downloaded songs back"),
        ] {
            let recording = blocked_for(reason);

            assert_eq!(recording.on(DETAILS_EVENT)[0]["phase"], phase);
            assert!(recording.on(crate::playback::PLAYER_EVENT)[0]["message"].as_str().unwrap().contains(words), "{phase}");
        }
    }

    #[test]
    fn a_blocked_read_stopped_by_the_user_does_not_go_on() {
        within_a_job(quiet(), Arc::new(DetailsState::default()), |host| {
            host.ctx.cancel_flag().store(true, Ordering::Relaxed);
            assert!(!host.blocked(Blocked::NoResponse));
        });
    }

    #[test]
    fn what_is_in_use_is_asked_of_playback() {
        within_a_job(Busy { looks_left: Cell::new(0), in_use: vec![3, 4] }, Arc::new(DetailsState::default()), |host| {
            assert_eq!(host.in_use(), [3, 4]);
        });
    }

    #[test]
    fn downloads_are_kept_on_a_list_that_survives_until_they_are_given_back() {
        within_a_job(quiet(), Arc::new(DetailsState::default()), |host| {
            let song = Path::new("/cloud/Artist/Album/Song.mp3");
            assert!(!host.fetched_earlier(song));

            host.fetching(song);
            assert!(host.fetched_earlier(song));
            // A sync's own entry for the same file is not this read's to give back.
            host.leftovers.add(&[PathBuf::from("/cloud/Other.mp3")], "an iPod", now_unix());
            assert!(!host.fetched_earlier(Path::new("/cloud/Other.mp3")));

            host.given_back(song);
            assert!(!host.fetched_earlier(song));
        });
    }

    #[test]
    fn the_notice_says_how_many_songs_were_read_skipped_and_not_found() {
        let summary = |read, skipped, missing| Ok::<_, String>(DetailsSummary { read, skipped, missing });
        assert_eq!(finished_notice(&summary(1_204, 0, 0), false), "Got details for 1204 songs");
        assert_eq!(finished_notice(&summary(1, 1, 0), false), "Got details for 1 song; 1 song couldn't be downloaded or read");
        assert_eq!(
            finished_notice(&summary(40, 3, 2), true),
            "Stopped after getting details for 40 songs; 3 songs couldn't be downloaded or read; 2 songs couldn't be found"
        );
        assert_eq!(finished_notice(&summary(0, 0, 0), false), "No songs need their details read");
        assert_eq!(finished_notice(&summary(0, 0, 4), false), "Got details for 0 songs; 4 songs couldn't be found");
        assert_eq!(finished_notice(&summary(0, 0, 0), true), "Stopped after getting details for 0 songs");
        assert_eq!(finished_notice(&Err::<DetailsSummary, _>("disk full"), false), "Couldn't get song details: disk full");
    }
}
