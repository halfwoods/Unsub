//! Running one speaker for as long as it is chosen: loading each song, watching where the speaker
//! has got to, moving on to the next, and telling the host what happens.
//!
//! The [`Driver`] is a state machine over a [`Receiver`]: [`Driver::handle`] takes the host's
//! commands, [`Driver::tick`] reads the speaker's status and [`Driver::apply_ready`] takes in the
//! songs that have been made ready. [`spawn`] gives it a thread of its own that does all three, so a
//! slow speaker never holds up the app.
//!
//! Making a song ready can take a while, when it has to be converted first. It is done on a helper
//! thread per song, so that the speaker is still being listened to and answered in the meantime: a
//! Chromecast hangs up on a sender that stops answering its pings.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::media::{Media, Receiver, State, Status};
use crate::{Error, Result};

/// How often the speaker is asked how it is getting on.
const TICK: Duration = Duration::from_millis(500);
/// How long the driver's thread waits for a command before looking at anything that is ready.
const SLICE: Duration = Duration::from_millis(100);
/// A speaker told to do something may go on reporting how it was for a moment; its word on the
/// matter is not taken until it has had this long to catch up.
const SETTLE: Duration = Duration::from_secs(4);
/// How long after a song is loaded, or the speaker moves on to the next, what the speaker reports
/// playing is not held against it: some renderers go on naming the previous song for a poll or two.
const URL_GRACE: Duration = Duration::from_secs(6);
/// A speaker that stops this close to the end of a song has played it out.
const END_SLACK_MS: u64 = 3_500;
/// How often the links of the current and queued songs are told they are still wanted. Well within how long a link lasts unused.
const KEEP_EVERY: Duration = Duration::from_secs(60);
/// Questions about status that fail without the speaker being gone, in a row, before it is given up on.
const MISSES_ALLOWED: u32 = 3;

/// What the host supplies: what a song is, and where from.
pub trait Provider: Send + Sync + 'static {
    /// Makes song `id` available to the speaker and describes it. This runs on a thread of its own
    /// and may take a while, as when a song the speaker can't play is converted first. `cancel` is
    /// set once the answer is no longer wanted, and a conversion should stop then.
    fn media(&self, id: i64, cancel: &AtomicBool) -> Result<Media>;

    /// The speaker has no more use for what [`Provider::media`] gave: its links can be closed.
    fn release(&self, media: &Media);

    /// Whether `url` is one of this provider's own links, however old. A speaker playing one of
    /// those is playing something of ours, though not necessarily what the driver thinks.
    fn serves(&self, url: &str) -> bool;

    /// The speaker still wants what [`Provider::media`] gave, though it has not asked for it for a
    /// while, as when it is paused or has the whole song buffered. Called about once a minute.
    fn keep(&self, media: &Media);
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Plays song `id` from `start_ms`, replacing whatever is playing and whatever was queued.
    Play {
        id: i64,
        start_ms: u64,
        paused: bool,
    },
    /// Has song `id` follow the current one, replacing any queued before.
    Queue {
        id: i64,
    },
    ClearQueue,
    Pause,
    Resume,
    Seek(u64),
    SetVolume(f32),
    Stop,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The speaker is playing song `id`: the one just asked for, or the queued one it moved on to.
    Started(i64),
    /// Song `id` played to its end.
    Ended(i64),
    /// Where the song is, about twice a second while it plays.
    Position {
        id: i64,
        position_ms: u64,
        duration_ms: u64,
    },
    /// Something about the playback changed: state, song or volume.
    State(Snapshot),
    /// A song could not be played. Casting goes on, stopped.
    Failed { id: Option<i64>, message: String },
    /// The speaker is gone, or another app has taken it over. The driver has stopped; nothing more
    /// will happen.
    Lost(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub state: State,
    pub id: Option<i64>,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// As last set or reported, 0.0 to 1.0.
    pub volume: Option<f32>,
}

struct Loaded {
    id: i64,
    media: Media,
    /// The speaker has been told to play it next by itself.
    preloaded: bool,
}

/// What a song being made ready is wanted for.
enum Wanted {
    Play { start_ms: u64, paused: bool },
    Next,
}

/// A song being made ready on a helper thread.
struct Job {
    id: i64,
    wanted: Wanted,
    /// Set when the answer is no longer wanted.
    cancel: Arc<AtomicBool>,
}

struct Ready {
    ticket: u64,
    result: Result<Media>,
}

/// Runs a job on a thread of its own.
type Runner = Box<dyn Fn(Box<dyn FnOnce() + Send>) + Send>;

fn on_a_thread(job: Box<dyn FnOnce() + Send>) {
    // A thread that can't start leaves its song never ready, which the listener finds out as a
    // speaker that does nothing; better than a driver that panics.
    let _ = thread::Builder::new()
        .name("unsub-cast-prepare".into())
        .spawn(job);
}

/// What makes a link the same link when a speaker reports it back: the secret in it, since a
/// speaker may rewrite the rest (the host, the extension, a query).
fn link_id(url: &str) -> &str {
    match url.rsplit_once("/link/") {
        Some((_, tail)) => tail.split(['.', '?', '#']).next().unwrap_or(tail),
        None => url,
    }
}

pub struct Driver {
    receiver: Box<dyn Receiver>,
    provider: Arc<dyn Provider>,
    emit: Box<dyn FnMut(Event) + Send>,
    current: Option<Loaded>,
    next: Option<Loaded>,
    jobs: HashMap<u64, Job>,
    tickets: u64,
    ready_to: Sender<Ready>,
    ready: mpsc::Receiver<Ready>,
    runner: Runner,
    snapshot: Snapshot,
    /// The state the speaker was last told to be in, until it says it is or the time is up.
    settling: Option<(State, Instant)>,
    /// When the current song was loaded, or the speaker moved on to it. Unlike `settling`, nothing
    /// that the speaker says clears it.
    loaded_at: Instant,
    misses: u32,
    kept: Instant,
    lost: bool,
}

impl Driver {
    pub fn new(
        receiver: Box<dyn Receiver>,
        provider: Arc<dyn Provider>,
        emit: impl FnMut(Event) + Send + 'static,
    ) -> Self {
        let (ready_to, ready) = mpsc::channel();
        Self {
            receiver,
            provider,
            emit: Box::new(emit),
            current: None,
            next: None,
            jobs: HashMap::new(),
            tickets: 0,
            ready_to,
            ready,
            runner: Box::new(on_a_thread),
            snapshot: Snapshot {
                state: State::Stopped,
                id: None,
                position_ms: 0,
                duration_ms: 0,
                volume: None,
            },
            settling: None,
            loaded_at: Instant::now(),
            misses: 0,
            kept: Instant::now(),
            lost: false,
        }
    }

    /// Makes songs ready where the driver runs, one at a time, rather than on threads of their own.
    #[cfg(test)]
    fn inline(mut self) -> Self {
        self.runner = Box::new(|job| job());
        self
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn is_lost(&self) -> bool {
        self.lost
    }

    pub fn handle(&mut self, command: Command) {
        if self.lost {
            return;
        }
        match command {
            Command::Play {
                id,
                start_ms,
                paused,
            } => self.play(id, start_ms, paused),
            Command::Queue { id } => self.queue(id),
            Command::ClearQueue => self.drop_next(),
            // A song on its way is what these are about, not the one it will replace.
            Command::Pause => {
                if !self.adjust_pending(|_, paused| *paused = true) {
                    self.set_state(State::Paused);
                }
            }
            Command::Resume => {
                if !self.adjust_pending(|_, paused| *paused = false) {
                    self.set_state(State::Playing);
                }
            }
            Command::Seek(position_ms) => {
                if self.adjust_pending(|start_ms, _| *start_ms = position_ms) {
                    // Applied when the song loads.
                } else if self.current.is_some() {
                    let result = self.receiver.seek(position_ms);
                    if self.check(result, self.snapshot.id).is_some() {
                        self.snapshot.position_ms = position_ms;
                        self.publish();
                    }
                }
            }
            Command::SetVolume(volume) => {
                let result = self.receiver.set_volume(volume);
                if self.check(result, None).is_some() {
                    self.snapshot.volume = Some(volume);
                    self.publish();
                }
            }
            Command::Stop => self.stop(),
        }
    }

    /// Takes in the songs that have been made ready since last time: a song to play is loaded, and a
    /// song to follow is offered to the speaker.
    pub fn apply_ready(&mut self) {
        while let Ok(ready) = self.ready.try_recv() {
            self.apply(ready);
        }
    }

    /// Reads the speaker's status and acts on it: a song that finished is followed by the next.
    pub fn tick(&mut self) {
        self.tick_at(Instant::now());
    }

    /// [`Driver::tick`] at the time `now`, which tests move along as polls go by.
    fn tick_at(&mut self, now: Instant) {
        if self.lost {
            return;
        }
        if now.saturating_duration_since(self.kept) >= KEEP_EVERY {
            self.kept = now;
            for loaded in self.current.iter().chain(&self.next) {
                self.provider.keep(&loaded.media);
            }
        }
        let status = match self.receiver.status() {
            Ok(status) => {
                self.misses = 0;
                status
            }
            Err(error) => {
                self.misses += 1;
                if error.is_lost() || self.misses >= MISSES_ALLOWED {
                    self.lose(&error);
                }
                return;
            }
        };
        if let Some(volume) = status.volume.filter(|volume| {
            self.snapshot
                .volume
                .is_none_or(|known| (known - volume).abs() > 0.005)
        }) {
            self.snapshot.volume = Some(volume);
            self.publish();
        }
        let Some(current) = &self.current else { return };

        let reported = status.url.as_deref().map(link_id);
        let is_ours = |loaded: &Loaded| Some(link_id(&loaded.media.url)) == reported;
        let moved_on = self
            .next
            .as_ref()
            .is_some_and(|next| is_ours(next) && !is_ours(current));
        // Only an address that is none of our own links is another app's: a link of ours that is not
        // the current song is a stale answer, such as a song a dropped queueing had already moved onto.
        let taken_over = now.saturating_duration_since(self.loaded_at) >= URL_GRACE
            && status.state != State::Stopped
            && status
                .url
                .as_deref()
                .is_some_and(|url| !self.provider.serves(url))
            && !is_ours(current)
            && !self.next.as_ref().is_some_and(is_ours);
        if moved_on {
            self.advance(&status, now);
        } else if taken_over {
            // What it plays now is not ours: another app has started something on the speaker.
            // Carrying on would overwrite its song's place with ours, and load our next song over it.
            self.lose(&Error::Lost(
                "another app has taken over the speaker".into(),
            ));
        } else if let Some(message) = status.error.clone() {
            let id = self.snapshot.id;
            self.fail(id, &message);
        } else {
            self.follow(&status, now);
        }
    }

    /// Stops playing and lets go of the speaker. What it was doing is what `snapshot` said before.
    pub fn finish(mut self) {
        self.cancel_jobs(|_| true);
        if !self.lost {
            let _ = self.receiver.stop();
            self.receiver.disconnect();
        }
        self.release_all();
    }

    /// Starts making song `id` ready.
    fn prepare(&mut self, id: i64, wanted: Wanted) {
        self.tickets += 1;
        let ticket = self.tickets;
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs.insert(
            ticket,
            Job {
                id,
                wanted,
                cancel: Arc::clone(&cancel),
            },
        );
        let (provider, ready_to) = (Arc::clone(&self.provider), self.ready_to.clone());
        (self.runner)(Box::new(move || {
            let result = provider.media(id, &cancel);
            // Nobody is listening once the driver is gone, and the song's links go with it.
            if let Err(mpsc::SendError(ready)) = ready_to.send(Ready { ticket, result })
                && let Ok(media) = ready.result
            {
                provider.release(&media);
            }
        }));
    }

    /// Stops wanting the jobs `which` picks. Their songs are released when they turn up.
    fn cancel_jobs(&mut self, which: impl Fn(&Job) -> bool) {
        self.jobs.retain(|_, job| {
            let unwanted = which(job);
            if unwanted {
                job.cancel.store(true, Ordering::Relaxed);
            }
            !unwanted
        });
    }

    fn playing_soon(&self) -> bool {
        self.jobs
            .values()
            .any(|job| matches!(job.wanted, Wanted::Play { .. }))
    }

    fn apply(&mut self, ready: Ready) {
        let Some(job) = self.jobs.remove(&ready.ticket) else {
            // Wanted once, and not any more.
            if let Ok(media) = ready.result {
                self.provider.release(&media);
            }
            return;
        };
        let media = match ready.result {
            Ok(media) if !self.lost => media,
            Ok(media) => return self.provider.release(&media),
            Err(error) => {
                return match job.wanted {
                    Wanted::Play { .. } => self.fail(Some(job.id), &error.to_string()),
                    Wanted::Next => (self.emit)(Event::Failed {
                        id: Some(job.id),
                        message: error.to_string(),
                    }),
                };
            }
        };
        match job.wanted {
            Wanted::Play { start_ms, paused } => self.load(job.id, media, start_ms, paused),
            Wanted::Next => self.follow_with(job.id, media),
        }
    }

    fn play(&mut self, id: i64, start_ms: u64, paused: bool) {
        self.cancel_jobs(|_| true);
        self.drop_next();
        self.prepare(id, Wanted::Play { start_ms, paused });
        if self.current.is_none() {
            // Nothing is playing in the meantime, so the song is shown as on its way.
            self.show_pending();
        }
    }

    /// The song being made ready to play, as a snapshot would show it, if there is one.
    fn pending_snapshot(&self) -> Option<Snapshot> {
        self.jobs.values().find_map(|job| match job.wanted {
            Wanted::Play { start_ms, paused } => Some(Snapshot {
                state: if paused {
                    State::Paused
                } else {
                    State::Buffering
                },
                id: Some(job.id),
                position_ms: start_ms,
                duration_ms: 0,
                volume: self.snapshot.volume,
            }),
            Wanted::Next => None,
        })
    }

    fn show_pending(&mut self) {
        if let Some(pending) = self.pending_snapshot() {
            self.snapshot = pending;
            self.publish();
        }
    }

    /// Changes where the song being made ready will start, or whether it will, and says whether
    /// there was one. With nothing else playing, what is shown follows.
    fn adjust_pending(&mut self, change: impl FnOnce(&mut u64, &mut bool)) -> bool {
        let Some(Wanted::Play { start_ms, paused }) = self
            .jobs
            .values_mut()
            .map(|job| &mut job.wanted)
            .find(|wanted| matches!(wanted, Wanted::Play { .. }))
        else {
            return false;
        };
        change(start_ms, paused);
        if self.current.is_none() {
            self.show_pending();
        }
        true
    }

    /// Where playing stood when the driver was asked to let go, for the next output to carry on
    /// from. A song on its way is the one to carry on with.
    pub fn handover(&self) -> Snapshot {
        match self.pending_snapshot() {
            Some(pending) => Snapshot {
                state: if pending.state == State::Paused {
                    State::Paused
                } else {
                    State::Playing
                },
                ..pending
            },
            None => self.snapshot.clone(),
        }
    }

    /// The song `id`, made ready, is loaded in place of whatever plays.
    fn load(&mut self, id: i64, media: Media, start_ms: u64, paused: bool) {
        if let Err(error) = self.receiver.load(&media, start_ms, !paused) {
            self.provider.release(&media);
            if error.is_lost() {
                return self.lose(&error);
            }
            return self.fail(Some(id), &error.to_string());
        }
        if let Some(replaced) = self.current.take() {
            self.provider.release(&replaced.media);
        }
        let state = if paused {
            State::Paused
        } else {
            State::Playing
        };
        self.settling = Some((state, Instant::now()));
        self.loaded_at = Instant::now();
        self.snapshot = Snapshot {
            state,
            id: Some(id),
            position_ms: start_ms,
            duration_ms: media.duration_ms,
            volume: self.snapshot.volume,
        };
        self.current = Some(Loaded {
            id,
            media,
            preloaded: false,
        });
        (self.emit)(Event::Started(id));
        self.publish();
    }

    fn queue(&mut self, id: i64) {
        let wanted_already = self.next.as_ref().is_some_and(|next| next.id == id)
            || self
                .jobs
                .values()
                .any(|job| matches!(job.wanted, Wanted::Next) && job.id == id);
        if wanted_already {
            return;
        }
        self.drop_next();
        // With nothing playing, or a song about to replace what is, there is nothing for it to
        // follow; the host asks again when the new song has started.
        if self.current.is_some() && !self.playing_soon() {
            self.prepare(id, Wanted::Next);
        }
    }

    /// The song `id` to follow the current one, made ready, and offered to the speaker.
    fn follow_with(&mut self, id: i64, media: Media) {
        if self.current.is_none() || self.playing_soon() {
            return self.provider.release(&media);
        }
        // A speaker that can't queue, or won't take this song, has it loaded when the current one ends.
        let preloaded = match self.receiver.preload(&media) {
            Ok(preloaded) => preloaded,
            Err(error) if error.is_lost() => {
                self.provider.release(&media);
                return self.lose(&error);
            }
            Err(_) => false,
        };
        self.next = Some(Loaded {
            id,
            media,
            preloaded,
        });
    }

    fn drop_next(&mut self) {
        self.cancel_jobs(|job| matches!(job.wanted, Wanted::Next));
        let Some(next) = self.next.take() else { return };
        if next.preloaded
            && let Err(error) = self.receiver.clear_preload()
            && error.is_lost()
        {
            self.lose(&error);
        }
        self.provider.release(&next.media);
    }

    fn release_all(&mut self) {
        for loaded in [self.current.take(), self.next.take()]
            .into_iter()
            .flatten()
        {
            self.provider.release(&loaded.media);
        }
    }

    fn set_state(&mut self, state: State) {
        if self.current.is_none() || self.snapshot.state == state {
            return;
        }
        let result = if state == State::Paused {
            self.receiver.pause()
        } else {
            self.receiver.play()
        };
        if self.check(result, self.snapshot.id).is_some() {
            self.settling = Some((state, Instant::now()));
            self.snapshot.state = state;
            self.publish();
        }
    }

    fn stop(&mut self) {
        if self.current.is_none() && self.next.is_none() && self.jobs.is_empty() {
            return;
        }
        self.cancel_jobs(|_| true);
        if self.current.is_some() {
            let result = self.receiver.stop();
            let _ = self.check(result, None);
        }
        self.release_all();
        self.settling = None;
        self.snapshot = Snapshot {
            state: State::Stopped,
            id: None,
            position_ms: 0,
            duration_ms: 0,
            volume: self.snapshot.volume,
        };
        self.publish();
    }

    /// Takes a command's outcome: `Some` if it worked. A speaker that is gone ends the driver; one
    /// that merely refused is reported and left as it is.
    fn check<T>(&mut self, result: Result<T>, id: Option<i64>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) if error.is_lost() => {
                self.lose(&error);
                None
            }
            Err(error) => {
                (self.emit)(Event::Failed {
                    id,
                    message: error.to_string(),
                });
                None
            }
        }
    }

    /// The current song can't go on. Whatever was queued is not played: something is wrong
    /// with the speaker, and it is for the listener to say what next.
    fn fail(&mut self, id: Option<i64>, message: &str) {
        self.cancel_jobs(|_| true);
        if self.current.is_some() {
            let _ = self.receiver.stop();
        }
        self.release_all();
        self.settling = None;
        self.snapshot = Snapshot {
            state: State::Stopped,
            id: None,
            position_ms: 0,
            duration_ms: 0,
            volume: self.snapshot.volume,
        };
        (self.emit)(Event::Failed {
            id,
            message: message.to_owned(),
        });
        self.publish();
    }

    fn lose(&mut self, error: &Error) {
        self.lost = true;
        self.cancel_jobs(|_| true);
        self.release_all();
        (self.emit)(Event::Lost(error.to_string()));
    }

    fn publish(&mut self) {
        (self.emit)(Event::State(self.snapshot.clone()));
    }

    /// The speaker moved on to the song queued behind the current one, as it was asked to.
    fn advance(&mut self, status: &Status, now: Instant) {
        let (Some(finished), Some(next)) = (self.current.take(), self.next.take()) else {
            return;
        };
        (self.emit)(Event::Ended(finished.id));
        self.provider.release(&finished.media);
        self.snapshot = Snapshot {
            state: if status.state == State::Paused {
                State::Paused
            } else {
                State::Playing
            },
            id: Some(next.id),
            position_ms: status.position_ms,
            duration_ms: next.media.duration_ms.max(status.duration_ms),
            volume: self.snapshot.volume,
        };
        self.settling = None;
        self.loaded_at = now;
        self.current = Some(next);
        (self.emit)(Event::Started(self.snapshot.id.unwrap_or_default()));
        self.publish();
    }

    /// The current song is over. Whatever is queued is loaded, if the speaker has not already
    /// started it by itself.
    fn finished(&mut self, now: Instant) {
        let Some(finished) = self.current.take() else {
            return;
        };
        (self.emit)(Event::Ended(finished.id));
        self.provider.release(&finished.media);
        // A song about to replace this one takes over from here: asking for it dropped the queue,
        // and nothing is queued behind it until it has started.
        let Some(next) = self.next.take() else {
            self.snapshot = self.pending_snapshot().unwrap_or(Snapshot {
                state: State::Stopped,
                id: None,
                position_ms: 0,
                duration_ms: 0,
                volume: self.snapshot.volume,
            });
            return self.publish();
        };
        match self.receiver.load(&next.media, 0, true) {
            Ok(()) => {
                self.settling = Some((State::Playing, now));
                self.loaded_at = now;
                self.snapshot = Snapshot {
                    state: State::Playing,
                    id: Some(next.id),
                    position_ms: 0,
                    duration_ms: next.media.duration_ms,
                    volume: self.snapshot.volume,
                };
                let id = next.id;
                self.current = Some(Loaded {
                    preloaded: false,
                    ..next
                });
                (self.emit)(Event::Started(id));
                self.publish();
            }
            Err(error) => {
                self.provider.release(&next.media);
                if error.is_lost() {
                    self.lose(&error);
                } else {
                    self.fail(Some(next.id), &error.to_string());
                }
            }
        }
    }

    fn follow(&mut self, status: &Status, now: Instant) {
        let settling = self
            .settling
            .filter(|(_, since)| now.saturating_duration_since(*since) < SETTLE);
        match status.state {
            State::Stopped => {
                let near_end = self.snapshot.duration_ms > 0
                    && self.snapshot.position_ms + END_SLACK_MS >= self.snapshot.duration_ms;
                if status.finished || (settling.is_none() && near_end) {
                    self.finished(now);
                } else if settling.is_none() {
                    // Someone else stopped the speaker.
                    self.release_all();
                    self.snapshot = Snapshot {
                        state: State::Stopped,
                        id: None,
                        position_ms: 0,
                        duration_ms: 0,
                        volume: self.snapshot.volume,
                    };
                    self.publish();
                }
            }
            reported => {
                let reported = if reported == State::Paused {
                    State::Paused
                } else {
                    State::Playing
                };
                let as_told = settling.is_none_or(|(told, _)| told == reported);
                if as_told {
                    self.settling = None;
                }
                let changed = as_told && reported != self.snapshot.state;
                if as_told {
                    self.snapshot.position_ms = status.position_ms;
                }
                if status.duration_ms > 0 {
                    self.snapshot.duration_ms = status.duration_ms;
                }
                if changed {
                    self.snapshot.state = reported;
                    self.publish();
                }
                if self.snapshot.state == State::Playing {
                    (self.emit)(Event::Position {
                        id: self.snapshot.id.unwrap_or_default(),
                        position_ms: self.snapshot.position_ms,
                        duration_ms: self.snapshot.duration_ms,
                    });
                }
            }
        }
    }
}

enum Message {
    Do(Command),
    Shutdown,
}

/// A running [`Driver`]'s thread.
pub struct Handle {
    messages: Sender<Message>,
    snapshot: Arc<Mutex<Snapshot>>,
    thread: Option<JoinHandle<Snapshot>>,
}

/// Gives `driver` a thread of its own, which carries out commands as they arrive, takes in songs
/// as they are made ready and reads the speaker's status every half second.
pub fn spawn(mut driver: Driver) -> std::io::Result<Handle> {
    let (messages, received) = mpsc::channel();
    let snapshot = Arc::new(Mutex::new(driver.snapshot().clone()));
    let published = Arc::clone(&snapshot);
    let thread = thread::Builder::new()
        .name("unsub-cast-driver".into())
        .spawn(move || {
            let mut last_tick = Instant::now();
            loop {
                match received.recv_timeout(SLICE) {
                    Ok(Message::Do(command)) => driver.handle(command),
                    Ok(Message::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                driver.apply_ready();
                if last_tick.elapsed() >= TICK {
                    last_tick = Instant::now();
                    driver.tick();
                }
                *published.lock().unwrap_or_else(PoisonError::into_inner) =
                    driver.snapshot().clone();
                if driver.is_lost() {
                    break;
                }
            }
            let last = driver.handover();
            driver.finish();
            last
        })?;
    Ok(Handle {
        messages,
        snapshot,
        thread: Some(thread),
    })
}

impl Handle {
    pub fn send(&self, command: Command) {
        // A send only fails once the driver has stopped because the speaker was lost, and the host has been told.
        let _ = self.messages.send(Message::Do(command));
    }

    /// As of the driver's last look at the speaker.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_guard().clone()
    }

    fn snapshot_guard(&self) -> MutexGuard<'_, Snapshot> {
        self.snapshot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stops playing and lets go of the speaker, and says what it was doing.
    pub fn shutdown(mut self) -> Snapshot {
        let _ = self.messages.send(Message::Shutdown);
        self.thread
            .take()
            .and_then(|thread| thread.join().ok())
            .unwrap_or_else(|| self.snapshot())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.messages.send(Message::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    use super::*;

    #[derive(Default)]
    struct Script {
        calls: Vec<String>,
        /// How many times the speaker has been asked how it is.
        asked: u32,
        statuses: VecDeque<Result<Status>>,
        /// What `status` says once the queue is empty.
        steady: Option<Status>,
        can_preload: bool,
        load_error: Option<fn() -> Error>,
        preload_error: Option<fn() -> Error>,
    }

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Script>>);

    impl Fake {
        fn script(&self) -> MutexGuard<'_, Script> {
            self.0.lock().unwrap()
        }

        fn calls(&self) -> Vec<String> {
            self.script().calls.clone()
        }

        fn says(&self, status: Status) {
            self.script().statuses.push_back(Ok(status));
        }

        fn fails(&self, error: Error) {
            self.script().statuses.push_back(Err(error));
        }
    }

    impl Receiver for Fake {
        fn load(&mut self, media: &Media, start_ms: u64, autoplay: bool) -> Result<()> {
            let mut script = self.script();
            script.calls.push(format!(
                "load {} @{start_ms} {}",
                media.title,
                if autoplay { "play" } else { "paused" }
            ));
            script.load_error.map_or(Ok(()), |error| Err(error()))
        }
        fn preload(&mut self, media: &Media) -> Result<bool> {
            let mut script = self.script();
            script.calls.push(format!("preload {}", media.title));
            script
                .preload_error
                .map_or(Ok(script.can_preload), |error| Err(error()))
        }
        fn clear_preload(&mut self) -> Result<()> {
            self.script().calls.push("clear_preload".into());
            Ok(())
        }
        fn play(&mut self) -> Result<()> {
            self.script().calls.push("play".into());
            Ok(())
        }
        fn pause(&mut self) -> Result<()> {
            self.script().calls.push("pause".into());
            Ok(())
        }
        fn seek(&mut self, position_ms: u64) -> Result<()> {
            self.script().calls.push(format!("seek {position_ms}"));
            Ok(())
        }
        fn set_volume(&mut self, volume: f32) -> Result<()> {
            self.script().calls.push(format!("volume {volume}"));
            Ok(())
        }
        fn status(&mut self) -> Result<Status> {
            let mut script = self.script();
            script.asked += 1;
            match script.statuses.pop_front() {
                Some(status) => status,
                None => script
                    .steady
                    .clone()
                    .ok_or_else(|| Error::Lost("no script".into())),
            }
        }
        fn stop(&mut self) -> Result<()> {
            self.script().calls.push("stop".into());
            Ok(())
        }
        fn disconnect(&mut self) {
            self.script().calls.push("disconnect".into());
        }
    }

    /// Songs named "song <id>", 200 seconds long, with their own link each time they are asked for.
    #[derive(Default)]
    struct Songs {
        issued: Mutex<u32>,
        released: Mutex<Vec<String>>,
        kept: Mutex<Vec<String>>,
        missing: Mutex<Vec<i64>>,
    }

    impl Songs {
        fn issued(&self) -> u32 {
            *self.issued.lock().unwrap()
        }
    }

    impl Provider for Songs {
        fn media(&self, id: i64, _cancel: &AtomicBool) -> Result<Media> {
            if self.missing.lock().unwrap().contains(&id) {
                return Err(Error::Refused(format!("song {id} can't be found")));
            }
            let mut issued = self.issued.lock().unwrap();
            *issued += 1;
            Ok(Media {
                url: format!("http://192.168.1.2:1/link/{id}-{issued}.flac"),
                mime: "audio/flac".into(),
                title: format!("song {id}"),
                artist: String::new(),
                album: String::new(),
                art_url: None,
                duration_ms: 200_000,
            })
        }

        fn serves(&self, url: &str) -> bool {
            url.starts_with("http://192.168.1.2:1/link/")
        }

        fn release(&self, media: &Media) {
            self.released.lock().unwrap().push(media.title.clone());
        }

        fn keep(&self, media: &Media) {
            self.kept.lock().unwrap().push(media.title.clone());
        }
    }

    struct Rig {
        driver: Driver,
        fake: Fake,
        songs: Arc<Songs>,
        events: mpsc::Receiver<Event>,
    }

    impl Rig {
        fn new(can_preload: bool) -> Self {
            let fake = Fake::default();
            fake.script().can_preload = can_preload;
            let songs = Arc::new(Songs::default());
            let (sender, events) = channel();
            let driver = Driver::new(Box::new(fake.clone()), songs.clone(), move |event| {
                sender.send(event).unwrap()
            })
            .inline();
            Self {
                driver,
                fake,
                songs,
                events,
            }
        }

        fn events(&self) -> Vec<Event> {
            self.events.try_iter().collect()
        }

        /// Gives the driver a command and takes in the song it asks to have made ready.
        fn run(&mut self, command: Command) {
            self.driver.handle(command);
            self.driver.apply_ready();
        }

        /// What a played-out speaker reports with the current song's link.
        fn url_of(&self, title: &str) -> String {
            self.driver
                .current
                .iter()
                .chain(self.driver.next.iter())
                .find(|l| l.media.title == title)
                .unwrap()
                .media
                .url
                .clone()
        }
    }

    fn status(state: State, position_ms: u64, url: &str) -> Status {
        Status {
            state,
            position_ms,
            duration_ms: 200_000,
            volume: None,
            url: Some(url.into()),
            finished: false,
            error: None,
        }
    }

    fn started(id: i64) -> Event {
        Event::Started(id)
    }

    fn snapshot(state: State, id: Option<i64>, position_ms: u64) -> Snapshot {
        Snapshot {
            state,
            id,
            position_ms,
            duration_ms: if id.is_some() { 200_000 } else { 0 },
            volume: None,
        }
    }

    #[test]
    fn playing_a_song_loads_it_where_asked_and_says_so() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 7,
            start_ms: 65_000,
            paused: false,
        });
        assert_eq!(rig.fake.calls(), ["load song 7 @65000 play"]);
        assert_eq!(
            rig.events(),
            [
                // First the song is shown as on its way, then it is playing.
                Event::State(Snapshot {
                    state: State::Buffering,
                    duration_ms: 0,
                    ..snapshot(State::Playing, Some(7), 65_000)
                }),
                started(7),
                Event::State(snapshot(State::Playing, Some(7), 65_000))
            ]
        );
    }

    #[test]
    fn a_song_handed_over_paused_is_loaded_paused() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 7,
            start_ms: 12_000,
            paused: true,
        });
        assert_eq!(rig.fake.calls(), ["load song 7 @12000 paused"]);
        assert_eq!(
            rig.events().last(),
            Some(&Event::State(snapshot(State::Paused, Some(7), 12_000)))
        );
    }

    #[test]
    fn position_events_follow_the_speaker_while_it_plays() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.events();
        let url = rig.url_of("song 1");
        rig.fake.says(status(State::Playing, 1_500, &url));
        rig.driver.tick();
        assert_eq!(
            rig.events(),
            [Event::Position {
                id: 1,
                position_ms: 1_500,
                duration_ms: 200_000
            }]
        );
    }

    #[test]
    fn a_queued_song_the_speaker_takes_over_is_started_without_a_reload() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.run(Command::Queue { id: 2 });
        rig.events();
        let second = rig.url_of("song 2");
        rig.fake.says(status(State::Playing, 400, &second));
        rig.driver.tick();

        assert_eq!(rig.fake.calls(), ["load song 1 @0 play", "preload song 2"]);
        assert_eq!(
            rig.events(),
            [
                Event::Ended(1),
                started(2),
                Event::State(snapshot(State::Playing, Some(2), 400))
            ]
        );
        assert_eq!(rig.songs.released.lock().unwrap().as_slice(), ["song 1"]);
    }

    #[test]
    fn repeating_one_song_is_told_apart_by_its_link() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.run(Command::Queue { id: 1 });
        rig.events();
        let again = rig.driver.next.as_ref().unwrap().media.url.clone();
        rig.fake.says(status(State::Playing, 100, &again));
        rig.driver.tick();
        assert_eq!(&rig.events()[..2], [Event::Ended(1), started(1)]);
    }

    #[test]
    fn a_speaker_that_cannot_queue_has_the_next_song_loaded_when_the_first_ends() {
        let mut rig = Rig::new(false);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.run(Command::Queue { id: 2 });
        rig.events();
        let first = rig.url_of("song 1");
        rig.fake.says(status(State::Playing, 199_000, &first));
        rig.fake.says(Status {
            finished: true,
            ..status(State::Stopped, 0, &first)
        });
        rig.driver.tick();
        rig.driver.tick();

        assert_eq!(rig.fake.calls().last().unwrap(), "load song 2 @0 play");
        let events = rig.events();
        assert!(events.contains(&Event::Ended(1)));
        assert!(events.contains(&started(2)));
        assert!(
            events.iter().position(|e| *e == Event::Ended(1))
                < events.iter().position(|e| *e == started(2))
        );
    }

    #[test]
    fn a_speaker_that_stops_near_the_end_has_played_the_song_out_even_if_it_does_not_say_so() {
        let mut rig = Rig::new(false);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.driver.settling = None;
        let url = rig.url_of("song 1");
        rig.fake.says(status(State::Playing, 198_000, &url));
        rig.fake.says(status(State::Stopped, 0, &url));
        rig.driver.tick();
        rig.events();
        rig.driver.tick();
        assert_eq!(
            rig.events(),
            [
                Event::Ended(1),
                Event::State(snapshot(State::Stopped, None, 0))
            ]
        );
    }

    #[test]
    fn the_last_song_ending_leaves_the_speaker_stopped() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.events();
        let url = rig.url_of("song 1");
        rig.fake.says(Status {
            finished: true,
            ..status(State::Stopped, 200_000, &url)
        });
        rig.driver.tick();
        assert_eq!(
            rig.events(),
            [
                Event::Ended(1),
                Event::State(snapshot(State::Stopped, None, 0))
            ]
        );
        assert_eq!(rig.driver.snapshot().id, None);
    }

    #[test]
    fn a_speaker_stopped_by_someone_else_mid_song_is_stopped_without_the_song_ending() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.driver.settling = None;
        rig.events();
        let url = rig.url_of("song 1");
        rig.fake.says(status(State::Stopped, 30_000, &url));
        rig.driver.tick();
        assert_eq!(
            rig.events(),
            [Event::State(snapshot(State::Stopped, None, 0))]
        );
        assert_eq!(
            rig.songs.released.lock().unwrap().as_slice(),
            ["song 1"],
            "its link and file are let go of"
        );
    }

    #[test]
    fn a_speaker_that_has_not_started_yet_is_given_time() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.events();
        let url = rig.url_of("song 1");
        rig.fake.says(status(State::Stopped, 0, &url));
        rig.fake.says(status(State::Buffering, 0, &url));
        rig.fake.says(status(State::Playing, 300, &url));
        rig.driver.tick();
        rig.driver.tick();
        rig.driver.tick();
        let events = rig.events();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Ended(_) | Event::State(_))),
            "{events:?}"
        );
        assert_eq!(rig.driver.snapshot().state, State::Playing);
    }

    #[test]
    fn pausing_is_not_undone_by_a_status_that_has_not_caught_up() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.driver.settling = None;
        rig.run(Command::Pause);
        rig.events();
        let url = rig.url_of("song 1");
        rig.fake.says(status(State::Playing, 2_000, &url));
        rig.fake.says(status(State::Paused, 2_000, &url));
        rig.driver.tick();
        assert_eq!(rig.driver.snapshot().state, State::Paused);
        assert!(
            rig.events().is_empty(),
            "no state change, no position while paused"
        );
        rig.driver.tick();
        assert_eq!(rig.driver.snapshot().state, State::Paused);
        assert_eq!(rig.fake.calls().last().unwrap(), "pause");
    }

    #[test]
    fn resume_seek_and_volume_reach_the_speaker_and_the_snapshot() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: true,
        });
        rig.run(Command::Resume);
        rig.run(Command::Seek(90_000));
        rig.run(Command::SetVolume(0.25));
        assert_eq!(rig.fake.calls()[1..], ["play", "seek 90000", "volume 0.25"]);
        let snapshot = rig.driver.snapshot();
        assert_eq!(
            (snapshot.state, snapshot.position_ms, snapshot.volume),
            (State::Playing, 90_000, Some(0.25))
        );
    }

    #[test]
    fn the_speakers_own_volume_is_picked_up() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.events();
        let url = rig.url_of("song 1");
        rig.fake.says(Status {
            volume: Some(0.8),
            ..status(State::Playing, 100, &url)
        });
        rig.driver.tick();
        assert_eq!(rig.driver.snapshot().volume, Some(0.8));
        assert!(
            rig.events()
                .iter()
                .any(|e| matches!(e, Event::State(s) if s.volume == Some(0.8)))
        );
    }

    #[test]
    fn queueing_the_same_song_twice_asks_the_speaker_once_and_clearing_takes_it_back() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.run(Command::Queue { id: 2 });
        rig.run(Command::Queue { id: 2 });
        assert_eq!(rig.fake.calls(), ["load song 1 @0 play", "preload song 2"]);
        rig.run(Command::ClearQueue);
        assert_eq!(rig.fake.calls().last().unwrap(), "clear_preload");
        assert_eq!(rig.songs.released.lock().unwrap().as_slice(), ["song 2"]);
        assert!(rig.driver.next.is_none());
    }

    #[test]
    fn replacing_the_queued_song_takes_back_the_first_and_closes_its_link() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.run(Command::Queue { id: 2 });
        rig.run(Command::Queue { id: 3 });
        assert_eq!(rig.fake.calls()[2..], ["clear_preload", "preload song 3"]);
        assert_eq!(rig.songs.released.lock().unwrap().as_slice(), ["song 2"]);
    }

    #[test]
    fn a_song_that_cannot_be_prepared_is_reported_and_nothing_is_loaded() {
        let mut rig = Rig::new(true);
        rig.songs.missing.lock().unwrap().push(9);
        rig.run(Command::Play {
            id: 9,
            start_ms: 0,
            paused: false,
        });
        assert!(rig.fake.calls().is_empty());
        assert!(
            rig.events().iter().any(|e| matches!(e, Event::Failed { id: Some(9), message } if message.contains("can't be found")))
        );
        assert_eq!(rig.driver.snapshot().state, State::Stopped);
    }

    #[test]
    fn a_song_the_speaker_refuses_is_reported_and_its_link_closed() {
        let mut rig = Rig::new(true);
        rig.fake.script().load_error = Some(|| Error::Refused("no".into()));
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        assert!(
            rig.events()
                .iter()
                .any(|e| matches!(e, Event::Failed { id: Some(1), .. }))
        );
        assert_eq!(rig.songs.released.lock().unwrap().as_slice(), ["song 1"]);
        assert!(!rig.driver.is_lost());
    }

    #[test]
    fn a_speaker_that_errors_while_playing_fails_the_song() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.events();
        let url = rig.url_of("song 1");
        rig.fake.says(Status {
            error: Some("could not play".into()),
            ..status(State::Stopped, 0, &url)
        });
        rig.driver.tick();
        assert!(
            matches!(&rig.events()[0], Event::Failed { id: Some(1), message } if message == "could not play")
        );
        assert_eq!(rig.driver.snapshot().state, State::Stopped);
    }

    #[test]
    fn a_failed_queueing_leaves_the_current_song_alone() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.songs.missing.lock().unwrap().push(2);
        rig.events();
        rig.run(Command::Queue { id: 2 });
        assert!(matches!(
            &rig.events()[0],
            Event::Failed { id: Some(2), .. }
        ));
        assert_eq!(rig.driver.snapshot().state, State::Playing);
    }

    #[test]
    fn a_lost_speaker_ends_the_driver_at_once_and_a_flaky_one_after_three_misses() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.events();
        rig.fake.fails(Error::Lost("gone".into()));
        rig.driver.tick();
        assert_eq!(rig.events(), [Event::Lost("gone".into())]);
        assert!(rig.driver.is_lost());
        rig.run(Command::Pause);
        assert!(rig.events().is_empty(), "a lost driver does nothing more");

        let mut flaky = Rig::new(true);
        flaky.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        flaky.events();
        for _ in 0..2 {
            flaky.fake.fails(Error::Protocol("garbled".into()));
            flaky.driver.tick();
        }
        assert!(!flaky.driver.is_lost());
        flaky.fake.fails(Error::Protocol("garbled".into()));
        flaky.driver.tick();
        assert!(flaky.driver.is_lost());
    }

    #[test]
    fn a_good_answer_forgives_the_misses_before_it() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        let url = rig.url_of("song 1");
        for _ in 0..2 {
            rig.fake.fails(Error::Protocol("garbled".into()));
            rig.driver.tick();
        }
        rig.fake.says(status(State::Playing, 100, &url));
        rig.driver.tick();
        for _ in 0..2 {
            rig.fake.fails(Error::Protocol("garbled".into()));
            rig.driver.tick();
        }
        assert!(!rig.driver.is_lost());
    }

    #[test]
    fn a_speaker_lost_while_loading_is_lost_not_failed() {
        let mut rig = Rig::new(true);
        rig.fake.script().load_error = Some(|| Error::Lost("gone".into()));
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        assert_eq!(rig.events().last(), Some(&Event::Lost("gone".into())));
        assert!(
            !rig.events()
                .iter()
                .any(|e| matches!(e, Event::Failed { .. }))
        );
    }

    #[test]
    fn the_links_of_the_current_and_queued_songs_are_kept_alive_now_and_then() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: true,
        });
        rig.run(Command::Queue { id: 2 });
        let url = rig.url_of("song 1");
        rig.fake.script().steady = Some(status(State::Paused, 0, &url));
        rig.driver.tick();
        assert!(rig.songs.kept.lock().unwrap().is_empty(), "not yet due");
        rig.driver.kept = Instant::now() - KEEP_EVERY;
        rig.driver.tick();
        assert_eq!(
            rig.songs.kept.lock().unwrap().as_slice(),
            ["song 1", "song 2"]
        );
        rig.driver.tick();
        assert_eq!(
            rig.songs.kept.lock().unwrap().len(),
            2,
            "and not again at once"
        );
    }

    #[test]
    fn stopping_stops_the_speaker_and_closes_every_link() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        rig.run(Command::Queue { id: 2 });
        rig.events();
        rig.run(Command::Stop);
        assert_eq!(rig.fake.calls().last().unwrap(), "stop");
        assert_eq!(
            rig.events(),
            [Event::State(snapshot(State::Stopped, None, 0))]
        );
        let mut released = rig.songs.released.lock().unwrap().clone();
        released.sort();
        assert_eq!(released, ["song 1", "song 2"]);
    }

    #[test]
    fn finishing_stops_disconnects_and_closes_the_links() {
        let mut rig = Rig::new(true);
        rig.run(Command::Play {
            id: 1,
            start_ms: 0,
            paused: false,
        });
        let (fake, songs) = (rig.fake.clone(), Arc::clone(&rig.songs));
        rig.driver.finish();
        assert_eq!(fake.calls()[1..], ["stop", "disconnect"]);
        assert_eq!(songs.released.lock().unwrap().as_slice(), ["song 1"]);
    }

    #[test]
    fn a_spawned_driver_runs_commands_and_reports_where_it_left_off() {
        let rig = Rig::new(true);
        let (fake, events) = (rig.fake.clone(), rig.events);
        fake.script().steady = Some(status(State::Playing, 5_000, "x"));
        let handle = spawn(rig.driver).unwrap();
        handle.send(Command::Play {
            id: 4,
            start_ms: 1_000,
            paused: false,
        });
        while events.recv_timeout(Duration::from_secs(5)).unwrap() != started(4) {}
        handle.send(Command::SetVolume(0.5));
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.snapshot().volume != Some(0.5) {
            assert!(
                Instant::now() < deadline,
                "the snapshot was never published"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let last = handle.shutdown();
        assert_eq!((last.id, last.volume), (Some(4), Some(0.5)));
        let calls = fake.calls();
        assert_eq!(calls[calls.len() - 2..], ["stop", "disconnect"]);
    }

    fn play(id: i64) -> Command {
        Command::Play {
            id,
            start_ms: 0,
            paused: false,
        }
    }

    /// A speaker that has been playing our first song for two polls, with the second queued behind it.
    fn playing_first_of_two(rig: &mut Rig, start: Instant) {
        rig.run(play(1));
        rig.run(Command::Queue { id: 2 });
        rig.events();
        let first = rig.url_of("song 1");
        for poll in 1..=2 {
            rig.fake.says(status(State::Playing, poll * 1_000, &first));
            rig.driver.tick_at(start + Duration::from_secs(poll));
        }
        assert!(!rig.driver.is_lost());
    }

    #[test]
    fn another_app_starting_something_on_the_speaker_ends_casting_and_leaves_its_song_alone() {
        let start = Instant::now();
        let mut rig = Rig::new(true);
        playing_first_of_two(&mut rig, start);
        rig.events();

        rig.fake.says(status(
            State::Playing,
            12_000,
            "x-sonos-spotify:spotify%3atrack%3a1",
        ));
        rig.driver.tick_at(start + Duration::from_secs(20));

        assert_eq!(
            rig.events(),
            [Event::Lost("another app has taken over the speaker".into())]
        );
        let mut released = rig.songs.released.lock().unwrap().clone();
        released.sort();
        assert_eq!(released, ["song 1", "song 2"]);
        let fake = rig.fake.clone();
        rig.driver.finish();
        let calls = fake.calls();
        assert!(
            !calls.contains(&"stop".to_owned()) && !calls.contains(&"disconnect".to_owned()),
            "their song is not stopped: {calls:?}"
        );
    }

    #[test]
    fn a_speaker_naming_the_previous_song_for_a_poll_or_two_after_a_next_is_not_taken_over() {
        let start = Instant::now();
        let mut rig = Rig::new(true);
        playing_first_of_two(&mut rig, start);
        let (first, second) = (rig.url_of("song 1"), rig.url_of("song 2"));

        // The speaker moves on to the second song, then goes on naming the first for two polls.
        rig.fake.says(status(State::Playing, 200, &second));
        rig.driver.tick_at(start + Duration::from_secs(3));
        assert!(rig.events().contains(&started(2)));
        for poll in 4..=5 {
            rig.fake.says(status(State::Playing, 500 * poll, &first));
            rig.driver.tick_at(start + Duration::from_secs(poll));
        }
        assert!(!rig.driver.is_lost(), "an old link of ours");

        // Even a renderer that words it as something we never served is given the same time.
        for poll in 5..=6 {
            rig.fake.says(status(
                State::Playing,
                500 * poll,
                "http://renamed/older.flac",
            ));
            rig.driver.tick_at(start + Duration::from_secs(poll));
        }
        assert!(!rig.driver.is_lost(), "within the grace after the change");
        rig.fake.says(status(State::Playing, 9_000, &second));
        rig.driver.tick_at(start + Duration::from_secs(7));
        assert!(!rig.driver.is_lost());
    }

    #[test]
    fn a_stale_link_of_ours_is_never_a_takeover_however_long_it_goes_on() {
        let start = Instant::now();
        let mut rig = Rig::new(true);
        playing_first_of_two(&mut rig, start);
        // The song a dropped queueing had already moved onto: ours, but no longer anything we track.
        let stale = "http://192.168.1.2:1/link/9-9.flac";
        for poll in [30, 31, 60] {
            rig.fake.says(status(State::Playing, poll * 1_000, stale));
            rig.driver.tick_at(start + Duration::from_secs(poll));
        }
        assert!(!rig.driver.is_lost());
    }

    #[test]
    fn a_foreign_address_is_a_takeover_once_the_grace_after_a_change_has_passed() {
        let start = Instant::now();
        let mut rig = Rig::new(true);
        playing_first_of_two(&mut rig, start);
        let foreign = "http://192.168.1.77:9000/other.mp3";
        rig.fake.says(status(State::Playing, 3_000, foreign));
        rig.driver.tick_at(start + Duration::from_secs(3));
        assert!(!rig.driver.is_lost(), "just after the song loaded");
        rig.fake.says(status(State::Playing, 9_000, foreign));
        rig.driver.tick_at(start + Duration::from_secs(9));
        assert!(rig.driver.is_lost());
    }

    #[test]
    fn a_song_the_speaker_loads_by_itself_when_the_last_ends_starts_its_own_grace() {
        let start = Instant::now();
        let mut rig = Rig::new(false);
        playing_first_of_two(&mut rig, start);
        let first = rig.url_of("song 1");
        // No queueing, so the driver loads the second song when the first has played out.
        rig.fake.says(Status {
            finished: true,
            ..status(State::Stopped, 0, &first)
        });
        rig.driver.tick_at(start + Duration::from_secs(30));
        assert!(rig.events().contains(&started(2)));
        rig.fake
            .says(status(State::Playing, 500, "http://renamed/older.flac"));
        rig.driver.tick_at(start + Duration::from_secs(31));
        assert!(
            !rig.driver.is_lost(),
            "the first song may still be what it names"
        );
        rig.fake
            .says(status(State::Playing, 8_000, "http://renamed/older.flac"));
        rig.driver.tick_at(start + Duration::from_secs(40));
        assert!(rig.driver.is_lost());
    }

    #[test]
    fn a_speaker_that_rewrites_the_address_it_was_given_is_still_playing_our_song() {
        let start = Instant::now();
        let mut rig = Rig::new(true);
        playing_first_of_two(&mut rig, start);
        rig.events();
        let secret = link_id(&rig.url_of("song 1")).to_owned();
        let rewritten = format!("http://speaker-view.local:80/link/{secret}.mp3?session=7");
        rig.fake.says(status(State::Playing, 3_000, &rewritten));
        rig.driver.tick_at(start + Duration::from_secs(30));
        assert!(!rig.driver.is_lost());
        assert!(matches!(&rig.events()[..], [Event::Position { id: 1, .. }]));

        // And the queued song taking over is just as recognisable.
        let next = link_id(&rig.url_of("song 2")).to_owned();
        let rewritten = format!("http://speaker-view.local:80/link/{next}.flac?x");
        rig.fake.says(status(State::Playing, 5, &rewritten));
        rig.driver.tick_at(start + Duration::from_secs(31));
        assert!(rig.events().contains(&started(2)));
    }

    #[test]
    fn a_stopped_speaker_reporting_something_else_is_stopped_not_taken_over() {
        let mut rig = Rig::new(true);
        rig.run(play(1));
        rig.driver.settling = None;
        rig.events();
        rig.fake
            .says(status(State::Stopped, 0, "http://elsewhere/other.flac"));
        rig.driver.tick();
        assert!(!rig.driver.is_lost());
        assert_eq!(rig.driver.snapshot().state, State::Stopped);
    }

    #[test]
    fn a_song_that_turns_up_after_it_stopped_being_wanted_is_released_not_loaded() {
        let mut rig = Rig::new(true);
        rig.driver.handle(play(1));
        rig.driver.handle(Command::Stop);
        rig.driver.apply_ready();
        assert!(rig.fake.calls().is_empty());
        assert_eq!(rig.songs.released.lock().unwrap().as_slice(), ["song 1"]);
        assert_eq!(rig.driver.snapshot().state, State::Stopped);
    }

    #[test]
    fn only_the_latest_song_asked_to_play_is_loaded() {
        let mut rig = Rig::new(true);
        rig.driver.handle(play(1));
        rig.driver.handle(play(2));
        rig.driver.apply_ready();
        assert_eq!(rig.fake.calls(), ["load song 2 @0 play"]);
        assert_eq!(rig.songs.released.lock().unwrap().as_slice(), ["song 1"]);
    }

    #[test]
    fn a_song_to_follow_when_nothing_plays_or_is_about_to_is_not_made_ready() {
        let mut rig = Rig::new(true);
        rig.driver.handle(Command::Queue { id: 2 });
        rig.driver.apply_ready();
        assert_eq!(rig.songs.issued(), 0);
    }

    #[test]
    fn a_song_to_follow_is_asked_for_again_once_the_song_to_play_has_started() {
        let mut rig = Rig::new(true);
        rig.driver.handle(play(1));
        rig.driver.handle(Command::Queue { id: 2 });
        rig.driver.apply_ready();
        assert_eq!(
            rig.songs.issued(),
            1,
            "nothing is made ready to follow a song on its way"
        );
        assert_eq!(rig.fake.calls(), ["load song 1 @0 play"]);

        rig.run(Command::Queue { id: 2 });
        assert_eq!(rig.fake.calls(), ["load song 1 @0 play", "preload song 2"]);
    }

    #[test]
    fn a_song_ending_while_another_is_on_its_way_is_followed_by_that_one() {
        let start = Instant::now();
        let mut rig = Rig::new(true);
        rig.run(play(1));
        rig.run(Command::Queue { id: 2 });
        rig.driver.handle(play(3));
        let first = rig.url_of("song 1");
        rig.events();

        rig.fake.says(Status {
            finished: true,
            ..status(State::Stopped, 0, &first)
        });
        rig.driver.tick_at(start + Duration::from_secs(1));
        assert_eq!(
            rig.events(),
            [
                Event::Ended(1),
                Event::State(Snapshot {
                    state: State::Buffering,
                    duration_ms: 0,
                    ..snapshot(State::Playing, Some(3), 0)
                })
            ],
            "the song on its way is shown, not a stopped speaker"
        );
        rig.driver.apply_ready();
        assert_eq!(
            rig.fake.calls().last().unwrap(),
            "load song 3 @0 play",
            "and the queued song was not loaded over it"
        );
        assert!(
            !rig.fake
                .calls()
                .iter()
                .any(|call| call == "load song 2 @0 play")
        );
    }

    #[test]
    fn pause_resume_and_seek_while_a_song_is_on_its_way_are_about_that_song() {
        let mut rig = Rig::new(true);
        rig.run(play(1));
        rig.driver.handle(play(2));
        rig.driver.handle(Command::Pause);
        rig.driver.handle(Command::Seek(90_000));
        assert_eq!(
            rig.fake.calls(),
            ["load song 1 @0 play"],
            "the song playing is left alone"
        );
        rig.driver.apply_ready();
        assert_eq!(
            rig.fake.calls().last().unwrap(),
            "load song 2 @90000 paused"
        );

        // Pausing and resuming before it loads leaves it to play.
        rig.driver.handle(play(3));
        rig.driver.handle(Command::Pause);
        rig.driver.handle(Command::Resume);
        rig.driver.apply_ready();
        assert_eq!(rig.fake.calls().last().unwrap(), "load song 3 @0 play");
    }

    #[test]
    fn a_song_on_its_way_to_an_idle_speaker_is_shown_as_paused_when_pause_is_pressed() {
        let mut rig = Rig::new(true);
        rig.driver.handle(Command::Play {
            id: 4,
            start_ms: 30_000,
            paused: false,
        });
        rig.events();
        rig.driver.handle(Command::Pause);
        assert_eq!(
            rig.events(),
            [Event::State(Snapshot {
                state: State::Paused,
                duration_ms: 0,
                ..snapshot(State::Paused, Some(4), 30_000)
            })]
        );
        rig.driver.handle(Command::Seek(45_000));
        assert_eq!(rig.driver.snapshot().position_ms, 45_000);
    }

    #[test]
    fn letting_go_with_a_song_on_its_way_hands_over_that_song() {
        let mut rig = Rig::new(true);
        rig.run(play(1));
        rig.driver.handle(Command::Play {
            id: 7,
            start_ms: 42_000,
            paused: true,
        });
        let handover = rig.driver.handover();
        assert_eq!(
            (handover.id, handover.position_ms, handover.state),
            (Some(7), 42_000, State::Paused)
        );
        rig.driver.handle(Command::Resume);
        assert_eq!(rig.driver.handover().state, State::Playing);

        rig.driver.apply_ready();
        let loaded = rig.driver.handover();
        assert_eq!((loaded.id, loaded.position_ms), (Some(7), 42_000));
    }

    /// Songs that are not ready until the test says so, as a slow conversion is not.
    #[derive(Default)]
    struct Slow {
        open: Mutex<bool>,
        opened: std::sync::Condvar,
        cancelled: Mutex<Vec<i64>>,
    }

    impl Slow {
        fn open(&self) {
            *self.open.lock().unwrap() = true;
            self.opened.notify_all();
        }
    }

    impl Provider for Slow {
        fn media(&self, id: i64, cancel: &AtomicBool) -> Result<Media> {
            let mut open = self.open.lock().unwrap();
            while !*open {
                if cancel.load(Ordering::Relaxed) {
                    self.cancelled.lock().unwrap().push(id);
                    return Err(Error::Refused("cancelled".into()));
                }
                open = self
                    .opened
                    .wait_timeout(open, Duration::from_millis(5))
                    .unwrap()
                    .0;
            }
            Ok(Media {
                url: format!("http://192.168.1.2:1/link/{id}.flac"),
                mime: "audio/flac".into(),
                title: format!("song {id}"),
                artist: String::new(),
                album: String::new(),
                art_url: None,
                duration_ms: 200_000,
            })
        }

        fn serves(&self, url: &str) -> bool {
            url.starts_with("http://192.168.1.2:1/link/")
        }

        fn release(&self, _media: &Media) {}

        fn keep(&self, _media: &Media) {}
    }

    fn start_slow() -> (Handle, Fake, Arc<Slow>, mpsc::Receiver<Event>) {
        let fake = Fake::default();
        fake.script().steady = Some(status(State::Stopped, 0, "none"));
        let slow = Arc::new(Slow::default());
        let (sender, events) = channel();
        let driver = Driver::new(Box::new(fake.clone()), slow.clone(), move |event| {
            let _ = sender.send(event);
        });
        (spawn(driver).unwrap(), fake, slow, events)
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "never happened: {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn the_speaker_is_still_listened_to_and_obeyed_while_a_song_is_being_made_ready() {
        let (handle, fake, slow, events) = start_slow();
        handle.send(play(1));
        handle.send(Command::SetVolume(0.4));
        wait_for("the volume reaching the speaker", || {
            fake.calls().contains(&"volume 0.4".to_owned())
        });
        wait_for("the speaker being asked how it is", || {
            fake.script().asked > 0
        });
        assert!(
            events.try_iter().all(|e| !matches!(e, Event::Started(_))),
            "the song is not ready yet"
        );

        slow.open();
        wait_for("the song starting", || {
            events.try_iter().any(|e| e == started(1))
        });
        assert_eq!(fake.calls(), ["volume 0.4", "load song 1 @0 play"]);
        handle.shutdown();
    }

    #[test]
    fn a_song_still_being_made_ready_when_another_is_asked_for_is_told_to_stop() {
        let (handle, fake, slow, events) = start_slow();
        handle.send(play(1));
        handle.send(play(2));
        wait_for("the first song being cancelled", || {
            slow.cancelled.lock().unwrap().contains(&1)
        });
        slow.open();
        wait_for("the second song starting", || {
            events.try_iter().any(|e| e == started(2))
        });
        assert_eq!(fake.calls(), ["load song 2 @0 play"]);
        handle.shutdown();
    }

    #[test]
    fn shutting_down_with_a_song_still_being_made_ready_says_which_song_it_was() {
        let (handle, _fake, slow, events) = start_slow();
        handle.send(Command::Play {
            id: 7,
            start_ms: 42_000,
            paused: true,
        });
        wait_for("the song being shown as on its way", || {
            events
                .try_iter()
                .any(|e| matches!(e, Event::State(s) if s.id == Some(7)))
        });
        let last = handle.shutdown();
        assert_eq!(
            (last.id, last.position_ms, last.state),
            (Some(7), 42_000, State::Paused)
        );
        wait_for("its preparation being cancelled", || {
            slow.cancelled.lock().unwrap().contains(&7)
        });
    }

    #[test]
    fn shutting_down_stops_a_song_still_being_made_ready() {
        let (handle, _fake, slow, _events) = start_slow();
        handle.send(play(1));
        thread::sleep(Duration::from_millis(50));
        handle.shutdown();
        wait_for("the preparation being cancelled", || {
            slow.cancelled.lock().unwrap().contains(&1)
        });
    }
}
