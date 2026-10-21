//! The plumbing the app's long-running work shares: a thread, a place in line, a flag that stops it, progress the window can
//! keep up with, and the events that say it has ended. Each job keeps its own event names and payloads; this makes sure they
//! go out the same way.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use unsub_library::Library;
use unsub_playback::PlayerEvent;

use crate::error::{CommandError, CommandResult};
use crate::library::LibraryState;
use crate::playback::PLAYER_EVENT;

/// Work that changes a song in microseconds, or copies bytes, reports far too often to tell the window.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);
/// A std mutex is not fair, so a thread that lets go and takes it again at once would be given it again every time
/// (a `yield_now` is not enough), and whoever waits (Get Info, a rating) would sleep through the whole run.
const BREATHER: Duration = Duration::from_millis(5);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Which job. Of a kind that says so, one runs at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Reading or bringing in a Music library or a streaming service's export. Two at once would duplicate each other's
    /// playlists and fight over the progress display.
    Import,
    CleanUp,
    /// A sync or a copy off an iPod, which share a place so the two can't run at once.
    Sync,
    Organize,
    Scan,
    Podcasts,
}

impl Kind {
    /// What a second one is told. Kinds without it wait their turn on a lock of their own instead.
    fn busy(self) -> Option<&'static str> {
        match self {
            Kind::Import => Some("Another import is still running."),
            Kind::CleanUp => Some("Clean Up Song Info is already running"),
            Kind::Sync => Some("Another sync or copy is still running."),
            Kind::Organize | Kind::Scan | Kind::Podcasts => None,
        }
    }
}

/// What a job is, and what it says to the window while it runs and when it ends.
pub struct Job {
    kind: Kind,
    resource: Option<String>,
    /// Where progress goes, and the `null` that ends it.
    event: &'static str,
    /// Shown as soon as the job starts, for work whose first progress comes a while after.
    start: Option<Value>,
    ends_with_null: bool,
    changed: Option<&'static str>,
}

impl Job {
    pub fn new(kind: Kind, event: &'static str) -> Self {
        Self { kind, resource: None, event, start: None, ends_with_null: true, changed: None }
    }

    /// The device the job is using, which then can't be taken away (see [`Jobs::is_busy_with`]).
    pub fn resource(mut self, id: &str) -> Self {
        self.resource = Some(id.to_owned());
        self
    }

    pub fn starting(mut self, status: impl Serialize) -> Self {
        self.start = serde_json::to_value(status).ok();
        self
    }

    /// For progress the window never expected a `null` after: it clears it some other way, or never does.
    pub fn open_ended(mut self) -> Self {
        self.ends_with_null = false;
        self
    }

    /// An event to send once the job is over, such as the library having changed.
    pub fn changed(mut self, event: &'static str) -> Self {
        self.changed = Some(event);
        self
    }
}

/// Where a job's events go: the window, or a recording in a test.
pub trait Events: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: Value);
}

impl Events for AppHandle {
    fn emit(&self, event: &str, payload: Value) {
        let _ = Emitter::emit(self, event, payload);
    }
}

/// What a running job holds: its flag, and the throttle on what it reports.
pub struct Ctx {
    events: Arc<dyn Events>,
    event: &'static str,
    cancel: Arc<AtomicBool>,
    every: Duration,
    pace: Mutex<Pace>,
}

#[derive(Default)]
struct Pace {
    sent: Option<Instant>,
    stage: u64,
}

impl Ctx {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// For the calls that watch the flag themselves.
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    /// Reports how far the job has got. The first report, the last (`done` reaching `total`) and the first of each
    /// `stage` always go; the rest only once `PROGRESS_EVERY` has passed since the last that did. A job that reports once
    /// per item, before the slow work on it, makes the item part of the stage so that no report is held back.
    pub fn progress(&self, done: u64, total: Option<u64>, stage: impl Hash, status: impl Serialize) {
        let mut hasher = DefaultHasher::new();
        stage.hash(&mut hasher);
        let stage = hasher.finish();
        {
            let mut pace = lock(&self.pace);
            let due = pace.sent.is_none_or(|sent| sent.elapsed() >= self.every) || pace.stage != stage || total == Some(done);
            if !due {
                return;
            }
            *pace = Pace { sent: Some(Instant::now()), stage };
        }
        self.emit(self.event, status);
    }

    /// Tells the user something once the job is over, in the LCD's words.
    pub fn notice(&self, message: impl Into<String>) {
        self.emit(PLAYER_EVENT, PlayerEvent::Notice { message: message.into() });
    }

    /// For an event of the job's own besides its progress.
    pub fn emit(&self, event: &str, payload: impl Serialize) {
        if let Ok(payload) = serde_json::to_value(payload) {
            self.events.emit(event, payload);
        }
    }
}

struct Running {
    id: u64,
    kind: Kind,
    resource: Option<String>,
    cancel: Arc<AtomicBool>,
}

/// Every job running, which the rest of the app can ask about.
#[derive(Clone, Default)]
pub struct Jobs {
    running: Arc<Mutex<Vec<Running>>>,
    ids: Arc<AtomicU64>,
}

/// A job that has been given its place. Dropping it ends the job, even by a panic: the place goes back first, as the
/// jobs did, so whatever reacts to the end can start the next one; then the window is told.
struct Open {
    jobs: Jobs,
    id: u64,
    ctx: Ctx,
    ends_with_null: bool,
    changed: Option<&'static str>,
}

impl Drop for Open {
    fn drop(&mut self) {
        lock(&self.jobs.running).retain(|running| running.id != self.id);
        if self.ends_with_null {
            self.ctx.emit(self.ctx.event, Value::Null);
        }
        if let Some(changed) = self.changed {
            self.ctx.emit(changed, Value::Null);
        }
    }
}

impl Jobs {
    fn open(&self, events: Arc<dyn Events>, job: Job) -> CommandResult<Open> {
        let cancel = Arc::new(AtomicBool::new(false));
        let id = self.ids.fetch_add(1, Ordering::Relaxed);
        {
            let mut running = lock(&self.running);
            if let Some(busy) = job.kind.busy()
                && running.iter().any(|other| other.kind == job.kind)
            {
                return Err(CommandError::from(busy.to_owned()));
            }
            running.push(Running { id, kind: job.kind, resource: job.resource, cancel: Arc::clone(&cancel) });
        }
        let ctx = Ctx { events, event: job.event, cancel, every: PROGRESS_EVERY, pace: Mutex::default() };
        // Built before the work is handed off, so the window is told by the time the command returns.
        let open = Open { jobs: self.clone(), id, ctx, ends_with_null: job.ends_with_null, changed: job.changed };
        if let Some(start) = job.start {
            open.ctx.events.emit(job.event, start);
        }
        Ok(open)
    }

    /// Runs `work` on a thread of its own, or says why it can't.
    pub fn start(&self, events: Arc<dyn Events>, job: Job, work: impl FnOnce(&Ctx) + Send + 'static) -> CommandResult<JoinHandle<()>> {
        let open = self.open(events, job)?;
        Ok(std::thread::spawn(move || work(&open.ctx)))
    }

    /// Stops the jobs of a kind at their next look at the flag.
    pub fn cancel(&self, kind: Kind) {
        for running in lock(&self.running).iter().filter(|running| running.kind == kind) {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Whether a job is using this device, which then can't be taken away.
    pub fn is_busy_with(&self, resource: &str) -> bool {
        lock(&self.running).iter().any(|running| running.resource.as_deref() == Some(resource))
    }
}

/// Runs `work` on a thread of its own and returns at once. The jobs of a kind that waits on a lock of its own can't be
/// refused, so callers of those ignore the result.
pub fn start(app: &AppHandle, job: Job, work: impl FnOnce(&AppHandle, &Ctx) + Send + 'static) -> CommandResult<()> {
    let handle = app.clone();
    app.state::<Jobs>().start(Arc::new(app.clone()), job, move |ctx| work(&handle, ctx))?;
    Ok(())
}

/// Runs `work` on the blocking pool for a command that waits for what it finds.
pub async fn run<T: Send + 'static>(
    app: &AppHandle,
    job: Job,
    work: impl FnOnce(&AppHandle, &Ctx) -> T + Send + 'static,
) -> CommandResult<T> {
    let handle = app.clone();
    let open = app.state::<Jobs>().open(Arc::new(app.clone()), job)?;
    tauri::async_runtime::spawn_blocking(move || work(&handle, &open.ctx)).await.map_err(|error| CommandError::from(error.to_string()))
}

/// One chunk of work on the library's writer, and then a breather so that whoever waits for the writer gets it.
pub fn write<T>(library: &LibraryState, change: impl FnOnce(&mut Library) -> unsub_library::Result<T>) -> CommandResult<T> {
    let result = library.with_writer(change);
    if result.is_ok() {
        std::thread::sleep(BREATHER);
    }
    result
}

#[tauri::command]
pub fn job_cancel(jobs: State<'_, Jobs>, kind: Kind) {
    jobs.cancel(kind);
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use serde_json::json;

    use super::*;

    /// Every event sent, in order.
    #[derive(Default)]
    struct Recording(Mutex<Vec<(String, Value)>>);

    impl Events for Recording {
        fn emit(&self, event: &str, payload: Value) {
            lock(&self.0).push((event.to_owned(), payload));
        }
    }

    impl Recording {
        fn sent(&self) -> Vec<(String, Value)> {
            lock(&self.0).clone()
        }

        fn on(&self, event: &str) -> Vec<Value> {
            self.sent().into_iter().filter(|(name, _)| name == event).map(|(_, payload)| payload).collect()
        }
    }

    fn ctx(recording: &Arc<Recording>, every: Duration) -> Ctx {
        Ctx {
            events: Arc::clone(recording) as Arc<dyn Events>,
            event: "job://progress",
            cancel: Arc::default(),
            every,
            pace: Mutex::default(),
        }
    }

    /// Starts `job` on a thread that waits to be told to finish, so the test can look at the job while it runs.
    fn start_held(jobs: &Jobs, recording: &Arc<Recording>, job: Job) -> (mpsc::Sender<()>, JoinHandle<()>) {
        let (finish, finished) = mpsc::channel::<()>();
        let handle = jobs
            .start(Arc::clone(recording) as Arc<dyn Events>, job, move |_| {
                let _ = finished.recv();
            })
            .unwrap();
        (finish, handle)
    }

    #[test]
    fn progress_always_sends_its_first_and_its_last_and_holds_back_the_rest() {
        let recording = Arc::new(Recording::default());
        let ctx = ctx(&recording, Duration::from_secs(3600));
        for done in 0..=10 {
            ctx.progress(done, Some(10), (), done);
        }
        assert_eq!(recording.on("job://progress"), [json!(0), json!(10)]);
    }

    #[test]
    fn progress_without_a_total_sends_its_first_and_then_only_as_often_as_it_is_allowed() {
        let recording = Arc::new(Recording::default());
        let slow = ctx(&recording, Duration::from_secs(3600));
        for done in 0..5 {
            slow.progress(done, None, (), done);
        }
        assert_eq!(recording.on("job://progress"), [json!(0)]);

        let every = Arc::new(Recording::default());
        let unthrottled = ctx(&every, Duration::ZERO);
        for done in 0..5 {
            unthrottled.progress(done, None, (), done);
        }
        assert_eq!(every.on("job://progress").len(), 5);
    }

    #[test]
    fn the_first_report_of_each_stage_goes_however_soon_it_follows() {
        let recording = Arc::new(Recording::default());
        let ctx = ctx(&recording, Duration::from_secs(3600));
        ctx.progress(0, Some(5), "reading", "reading 0");
        ctx.progress(1, Some(5), "reading", "reading 1");
        ctx.progress(0, Some(5), "writing", "writing 0");
        ctx.progress(1, Some(5), "writing", "writing 1");
        ctx.progress(0, Some(5), "reading", "reading again");
        assert_eq!(recording.on("job://progress"), [json!("reading 0"), json!("writing 0"), json!("reading again")]);
    }

    #[test]
    fn a_report_made_before_slow_work_on_an_item_is_not_held_back_by_the_quick_item_before_it() {
        let recording = Arc::new(Recording::default());
        let ctx = ctx(&recording, Duration::from_secs(3600));
        // One report per song, each naming the song it is about to work on, as a sync makes them.
        for song in 0..4u64 {
            ctx.progress(song, Some(4), ("copying", song), format!("song {song}"));
        }
        assert_eq!(recording.on("job://progress"), [json!("song 0"), json!("song 1"), json!("song 2"), json!("song 3")]);

        let reports_within_a_song = Arc::new(Recording::default());
        let ctx = self::ctx(&reports_within_a_song, Duration::from_secs(3600));
        for bytes in 0..4u64 {
            ctx.progress(bytes, Some(100), ("copying", 7u64), bytes);
        }
        assert_eq!(reports_within_a_song.on("job://progress"), [json!(0)], "finer reports within one song are still held back");
    }

    #[test]
    fn a_job_says_where_it_stands_at_once_and_ends_with_null_and_then_what_changed() {
        let jobs = Jobs::default();
        let recording = Arc::new(Recording::default());
        let job = Job::new(Kind::CleanUp, "job://progress").starting(json!({ "stage": "reading" })).changed("library://changed");
        let handle = jobs
            .start(Arc::clone(&recording) as Arc<dyn Events>, job, |ctx| {
                ctx.progress(1, Some(2), (), json!({ "done": 1 }));
                ctx.notice("All done");
            })
            .unwrap();
        handle.join().unwrap();

        let events: Vec<String> = recording.sent().into_iter().map(|(name, _)| name).collect();
        assert_eq!(events, ["job://progress", "job://progress", PLAYER_EVENT, "job://progress", "library://changed"]);
        let progress = recording.on("job://progress");
        assert_eq!(progress, [json!({ "stage": "reading" }), json!({ "done": 1 }), Value::Null]);
        assert_eq!(recording.on(PLAYER_EVENT), [json!({ "type": "notice", "message": "All done" })]);
    }

    #[test]
    fn progress_the_window_never_expected_a_null_after_is_left_open() {
        let jobs = Jobs::default();
        let recording = Arc::new(Recording::default());
        let job = Job::new(Kind::Scan, "job://progress").open_ended();
        jobs.start(Arc::clone(&recording) as Arc<dyn Events>, job, |ctx| ctx.progress(0, None, (), 7)).unwrap().join().unwrap();
        assert_eq!(recording.on("job://progress"), [json!(7)]);
    }

    #[test]
    fn a_kind_that_allows_one_at_a_time_turns_a_second_away_until_the_first_is_over() {
        let jobs = Jobs::default();
        let recording = Arc::new(Recording::default());
        let (finish, handle) = start_held(&jobs, &recording, Job::new(Kind::Sync, "job://progress"));

        let refused = jobs.start(Arc::clone(&recording) as Arc<dyn Events>, Job::new(Kind::Sync, "job://progress"), |_| {});
        assert_eq!(refused.unwrap_err().to_string(), "Another sync or copy is still running.");
        // Another kind has a place of its own.
        let other = jobs.start(Arc::clone(&recording) as Arc<dyn Events>, Job::new(Kind::CleanUp, "job://other"), |_| {});
        other.unwrap().join().unwrap();

        finish.send(()).unwrap();
        handle.join().unwrap();
        let again = jobs.start(Arc::clone(&recording) as Arc<dyn Events>, Job::new(Kind::Sync, "job://progress"), |_| {});
        again.unwrap().join().unwrap();
    }

    #[test]
    fn a_refused_job_says_nothing_to_the_window() {
        let jobs = Jobs::default();
        let recording = Arc::new(Recording::default());
        let (finish, handle) = start_held(&jobs, &recording, Job::new(Kind::Import, "job://progress"));
        let before = recording.sent().len();

        let refused = Job::new(Kind::Import, "job://progress").starting("again").changed("library://changed");
        assert!(jobs.start(Arc::clone(&recording) as Arc<dyn Events>, refused, |_| {}).is_err());

        assert_eq!(recording.sent().len(), before);
        finish.send(()).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn a_kind_that_waits_on_a_lock_of_its_own_runs_alongside_itself() {
        let jobs = Jobs::default();
        let recording = Arc::new(Recording::default());
        let (finish_first, first) = start_held(&jobs, &recording, Job::new(Kind::Scan, "job://progress"));
        let (finish_second, second) = start_held(&jobs, &recording, Job::new(Kind::Scan, "job://progress"));
        finish_first.send(()).unwrap();
        finish_second.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();
    }

    #[test]
    fn a_job_holds_its_device_until_it_is_over() {
        let jobs = Jobs::default();
        let recording = Arc::new(Recording::default());
        assert!(!jobs.is_busy_with("ipod"));

        let (finish, handle) = start_held(&jobs, &recording, Job::new(Kind::Sync, "job://progress").resource("ipod"));

        assert!(jobs.is_busy_with("ipod") && !jobs.is_busy_with("another"));
        finish.send(()).unwrap();
        handle.join().unwrap();
        assert!(!jobs.is_busy_with("ipod"));
    }

    #[test]
    fn cancelling_a_kind_stops_its_jobs_and_no_others() {
        let jobs = Jobs::default();
        let recording = Arc::new(Recording::default());
        let (seen, stopped) = mpsc::channel();
        let (finish, finished) = mpsc::channel::<()>();
        let watching = jobs
            .start(Arc::clone(&recording) as Arc<dyn Events>, Job::new(Kind::CleanUp, "job://progress"), move |ctx| {
                assert!(!ctx.cancelled());
                seen.send(()).unwrap();
                let _ = finished.recv();
                assert!(ctx.cancelled() && ctx.cancel_flag().load(Ordering::Relaxed));
            })
            .unwrap();
        let (finish_other, other) = start_held(&jobs, &recording, Job::new(Kind::Organize, "job://other"));
        stopped.recv().unwrap();

        jobs.cancel(Kind::CleanUp);
        finish.send(()).unwrap();
        watching.join().unwrap();

        let flags: Vec<bool> = lock(&jobs.running).iter().map(|running| running.cancel.load(Ordering::Relaxed)).collect();
        assert_eq!(flags, [false], "only the organize is left running, and it was not stopped");
        finish_other.send(()).unwrap();
        other.join().unwrap();
    }

    #[test]
    fn a_job_that_panics_still_gives_back_its_place_and_tells_the_window_it_is_over() {
        let jobs = Jobs::default();
        let recording = Arc::new(Recording::default());
        let job = Job::new(Kind::Sync, "job://progress").resource("ipod").changed("library://changed");
        let handle = jobs.start(Arc::clone(&recording) as Arc<dyn Events>, job, |_| panic!("on purpose")).unwrap();

        assert!(handle.join().is_err());

        assert!(!jobs.is_busy_with("ipod"));
        assert_eq!(recording.on("job://progress"), [Value::Null]);
        assert_eq!(recording.on("library://changed"), [Value::Null]);
        assert!(jobs.start(Arc::clone(&recording) as Arc<dyn Events>, Job::new(Kind::Sync, "job://progress"), |_| {}).is_ok());
    }

    #[test]
    fn a_chunk_of_writer_work_gives_back_what_it_found_and_its_error() {
        let dir = tempfile::tempdir().unwrap();
        let library = LibraryState::open(dir.path().join("library.db"), dir.path().join("art")).unwrap();
        assert_eq!(write(&library, |_| Ok(7)).unwrap(), 7);
        let failed = write(&library, |_| Err::<(), _>(unsub_library::Error::NoMediaFolder));
        assert_eq!(failed.unwrap_err().to_string(), unsub_library::Error::NoMediaFolder.to_string());
    }
}
