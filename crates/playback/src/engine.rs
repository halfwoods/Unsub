use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::decoder::Decoder;
use crate::device_state::Saved;
use crate::gain::Volume;
use crate::output::{Format, Output, Sink};
use crate::player::{PlaybackState, PlaybackStatus, PlayerEvent};
use crate::resample::Resampler;
use crate::timeline::{Timeline, TrackInfo};
use crate::{Error, Result};

/// How often the engine tops up the ring, notices track boundaries and refreshes the status.
const TICK: Duration = Duration::from_millis(10);

const POSITION_INTERVAL: Duration = Duration::from_millis(250);

/// The device still holds up to a buffer of audio the callback has already handed over.
const DRAIN_GRACE: Duration = Duration::from_millis(50);

pub(crate) enum Command {
    Play(i64, PathBuf),
    QueueNext(i64, PathBuf),
    ClearNext,
    Pause,
    Resume,
    Stop,
    Seek(u64),
    /// Sent by the stream's error callback, which must not touch the stream itself.
    OutputFailed {
        generation: u64,
        error: cpal::Error,
    },
    Shutdown,
}

/// How the engine opens files; tests swap it to inject faults.
type Opener = fn(&Path) -> Result<Decoder>;

/// A track that is opened and ready to decode.
struct Queued {
    track: TrackInfo,
    decoder: Decoder,
}

impl Queued {
    fn open(open: Opener, id: i64, path: PathBuf) -> Result<Queued> {
        let decoder = open(&path)?;
        let track = TrackInfo {
            id,
            path,
            duration_ms: decoder.duration_ms().unwrap_or(0),
            sample_rate: decoder.sample_rate(),
        };
        Ok(Queued { track, decoder })
    }
}

/// Decoder output on its way to the ring: converted to the device format and staged until the
/// ring has room.
struct Feed {
    decoder: Decoder,
    source: Format,
    resampler: Option<Resampler>,
    staging: Vec<f32>,
    /// Samples of `staging` already handed to the ring.
    staged: usize,
    /// Frames produced since the stream began, in the same index space as the consumed counter.
    produced: u64,
    out_channels: usize,
    eof: bool,
}

impl Feed {
    fn new(decoder: Decoder, output: Format, produced: u64) -> Result<Feed> {
        let source = Format {
            sample_rate: decoder.sample_rate(),
            channels: decoder.channels(),
        };
        let resampler = (source != output)
            .then(|| {
                Resampler::new(
                    source.sample_rate,
                    output.sample_rate,
                    source.channels,
                    output.channels,
                )
            })
            .transpose()?;
        Ok(Feed {
            decoder,
            source,
            resampler,
            staging: Vec::new(),
            staged: 0,
            produced,
            out_channels: usize::from(output.channels),
            eof: false,
        })
    }

    /// Whether `decoder` can follow this one through the same stream and converter.
    fn accepts(&self, decoder: &Decoder) -> bool {
        decoder.sample_rate() == self.source.sample_rate
            && decoder.channels() == self.source.channels
    }

    fn is_staging(&self) -> bool {
        self.staged < self.staging.len()
    }

    fn drain_into(&mut self, sink: &mut impl Sink) {
        self.staged += sink.push(&self.staging[self.staged..]);
        if !self.is_staging() {
            self.staging.clear();
            self.staged = 0;
        }
    }

    fn refill(&mut self) -> Result<()> {
        let before = self.staging.len();
        match self.decoder.next_chunk()? {
            Some(chunk) => match self.resampler.as_mut() {
                Some(resampler) => resampler.push(chunk, &mut self.staging)?,
                None => self.staging.extend_from_slice(chunk),
            },
            None => {
                // Each track is flushed on its own so the boundary frame index stays exact.
                if let Some(resampler) = self.resampler.as_mut() {
                    resampler.finish(&mut self.staging)?;
                }
                self.eof = true;
            }
        }
        self.produced += ((self.staging.len() - before) / self.out_channels) as u64;
        Ok(())
    }

    /// Decodes into `sink` until it is full, handing over to `next` when this track ends.
    fn fill(
        &mut self,
        sink: &mut impl Sink,
        timeline: &mut Timeline,
        next: &mut Option<Queued>,
    ) -> Result<()> {
        loop {
            self.drain_into(sink);
            if self.is_staging() {
                return Ok(());
            }
            if !self.eof {
                self.refill()?;
                continue;
            }
            // One handoff at a time keeps seeks and queue changes simple: see `Engine::reposition`.
            if timeline.upcoming().is_some() {
                return Ok(());
            }
            let Some(queued) = next.take_if(|queued| self.accepts(&queued.decoder)) else {
                return Ok(());
            };
            timeline.schedule(self.produced, queued.track);
            self.continue_with(queued.decoder);
        }
    }

    /// Continues with the next track straight after the current one's last frame.
    fn continue_with(&mut self, decoder: Decoder) {
        self.decoder = decoder;
        self.eof = false;
    }

    /// Discards everything decoded so far; the ring must already be empty at stream frame `frame`.
    fn rewind(&mut self, frame: u64) {
        self.staging.clear();
        self.staged = 0;
        self.produced = frame;
        self.eof = false;
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
    }
}

/// One open output stream and the track timeline playing through it.
struct Session {
    output: Output,
    feed: Feed,
    timeline: Timeline,
}

impl Session {
    fn open(
        queued: Queued,
        volume: Arc<Volume>,
        on_failure: impl Fn(cpal::Error) + Send + 'static,
    ) -> Result<Session> {
        let Queued { track, decoder } = queued;
        let source = Format {
            sample_rate: decoder.sample_rate(),
            channels: decoder.channels(),
        };
        let output = Output::open(source, volume, on_failure)?;
        let format = output.format();
        Ok(Session {
            feed: Feed::new(decoder, format, 0)?,
            timeline: Timeline::new(format.sample_rate, track, 0, 0),
            output,
        })
    }

    /// Decodes into the ring until it is full, handing over to `next` when this track ends.
    fn fill(&mut self, next: &mut Option<Queued>) -> Result<()> {
        self.feed.fill(&mut self.output, &mut self.timeline, next)
    }

    /// Restarts on a new track after the output has been flushed.
    fn restart(&mut self, queued: Queued) {
        let frame = self.output.consumed();
        self.feed.decoder = queued.decoder;
        self.feed.rewind(frame);
        self.timeline = Timeline::new(self.output.format().sample_rate, queued.track, frame, 0);
    }

    fn feeding_id(&self) -> i64 {
        self.timeline
            .upcoming()
            .unwrap_or_else(|| self.timeline.current())
            .id
    }

    /// Everything decoded has been heard and nothing follows.
    fn finished(&self) -> bool {
        self.feed.eof
            && !self.feed.is_staging()
            && self.timeline.upcoming().is_none()
            && self.output.consumed() >= self.feed.produced
    }
}

pub(crate) struct Engine {
    commands: Receiver<Command>,
    sender: Sender<Command>,
    emit: Arc<dyn Fn(PlayerEvent) + Send + Sync>,
    volume: Arc<Volume>,
    published: Arc<Mutex<PlaybackStatus>>,
    open: Opener,
    /// The output device as it was before this engine first changed it, restored once playback
    /// stops. Kept across format changes within one run of playback.
    saved_device: Option<Saved>,
    state: PlaybackState,
    session: Option<Session>,
    next: Option<Queued>,
    /// Identifies the current stream so a late error from a replaced one is ignored.
    generation: u64,
    next_position_event: Instant,
}

impl Engine {
    pub(crate) fn new(
        commands: Receiver<Command>,
        sender: Sender<Command>,
        emit: Arc<dyn Fn(PlayerEvent) + Send + Sync>,
        volume: Arc<Volume>,
        published: Arc<Mutex<PlaybackStatus>>,
    ) -> Self {
        Engine {
            commands,
            sender,
            emit,
            volume,
            published,
            open: Decoder::open,
            saved_device: None,
            state: PlaybackState::Stopped,
            session: None,
            next: None,
            generation: 0,
            next_position_event: Instant::now(),
        }
    }

    pub(crate) fn run(mut self) {
        'engine: loop {
            let first = if self.session.is_some() {
                match self.commands.recv_timeout(TICK) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                // Nothing to feed, so sleep until the next command rather than polling.
                match self.commands.recv() {
                    Ok(command) => Some(command),
                    Err(_) => break,
                }
            };

            for command in first
                .into_iter()
                .chain(self.commands.try_iter().collect::<Vec<_>>())
            {
                if self.handle(command).is_break() {
                    break 'engine;
                }
            }
            self.tick();
        }
    }

    fn handle(&mut self, command: Command) -> ControlFlow<()> {
        match command {
            Command::Play(id, path) => self.play(id, path),
            Command::QueueNext(id, path) => self.queue_next(id, path),
            Command::ClearNext => self.clear_next(),
            Command::Pause => self.set_paused(true),
            Command::Resume => self.set_paused(false),
            Command::Stop => self.stop(),
            Command::Seek(position_ms) => self.seek(position_ms),
            Command::OutputFailed { generation, error } if generation == self.generation => {
                self.fail(self.audible_id(), Error::Output(error));
            }
            Command::OutputFailed { .. } => {}
            Command::Shutdown => return ControlFlow::Break(()),
        }
        ControlFlow::Continue(())
    }

    fn play(&mut self, id: i64, path: PathBuf) {
        self.next = None;
        let started = Queued::open(self.open, id, path).and_then(|queued| self.start(queued));
        if let Err(error) = started {
            self.fail(Some(id), error);
        }
    }

    fn start(&mut self, queued: Queued) -> Result<()> {
        let id = queued.track.id;
        match self
            .session
            .as_mut()
            .filter(|session| session.feed.accepts(&queued.decoder))
        {
            // Same format: keep the device open so there is no reopen pop.
            Some(session) => {
                session.output.flush()?;
                session.restart(queued);
                session.output.set_paused(false);
                session.fill(&mut self.next)?;
            }
            None => {
                self.release();
                if self.saved_device.is_none() {
                    self.saved_device = Saved::capture();
                }
                self.generation += 1;
                let (sender, generation) = (self.sender.clone(), self.generation);
                let on_failure = move |error| {
                    let _ = sender.send(Command::OutputFailed { generation, error });
                };
                let mut session = Session::open(queued, Arc::clone(&self.volume), on_failure)?;
                session.fill(&mut self.next)?;
                // The ring is primed first so the first callback already has audio.
                session.output.start()?;
                self.session = Some(session);
            }
        }
        self.state = PlaybackState::Playing;
        self.next_position_event = Instant::now();
        (self.emit)(PlayerEvent::TrackStarted { track_id: id });
        self.announce();
        Ok(())
    }

    fn queue_next(&mut self, id: i64, path: PathBuf) {
        match Queued::open(self.open, id, path) {
            Ok(queued) => {
                self.next = Some(queued);
                // A handoff already decoded into the ring cannot be unpicked, so rewind to the
                // current position and let the new queue entry be decoded afresh.
                self.drop_upcoming();
            }
            Err(error) => {
                self.next = None;
                self.drop_upcoming();
                self.report(Some(id), &error);
            }
        }
    }

    fn clear_next(&mut self) {
        self.next = None;
        self.drop_upcoming();
    }

    fn drop_upcoming(&mut self) {
        let pending = self
            .session
            .as_ref()
            .is_some_and(|session| session.timeline.upcoming().is_some());
        if pending && let Err(error) = self.reposition(None, false) {
            self.fail(self.audible_id(), error);
        }
    }

    fn set_paused(&mut self, paused: bool) {
        let (from, to) = if paused {
            (PlaybackState::Playing, PlaybackState::Paused)
        } else {
            (PlaybackState::Paused, PlaybackState::Playing)
        };
        if self.state != from {
            return;
        }
        if let Some(session) = &self.session {
            session.output.set_paused(paused);
        }
        self.state = to;
        self.announce();
    }

    fn stop(&mut self) {
        self.next = None;
        if self.state == PlaybackState::Stopped {
            return;
        }
        self.release();
        self.state = PlaybackState::Stopped;
        self.announce();
        self.restore_device();
    }

    fn seek(&mut self, position_ms: u64) {
        if let Err(error) = self.reposition(Some(position_ms), true) {
            self.fail(self.audible_id(), error);
        }
    }

    /// Re-decodes the audible track from `target_ms`, or from where it is now when `None`.
    ///
    /// Decoding can be a whole handoff ahead of what is heard, so when one is pending the
    /// audible track's decoder is already gone and must be reopened. The track decoded ahead is
    /// put back in the queue when `keep_upcoming` is set.
    fn reposition(&mut self, target_ms: Option<u64>, keep_upcoming: bool) -> Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        // Fade out first: the position is read once the fade has finished, so a restart at the
        // current position resumes exactly where the listener stopped hearing.
        session.output.flush()?;
        let frame = session.output.consumed();
        let audible = session.timeline.current().clone();
        let position_ms = target_ms.unwrap_or_else(|| session.timeline.position_ms(frame));
        let position_ms = match audible.duration_ms {
            0 => position_ms,
            duration => position_ms.min(duration.saturating_sub(1)),
        };

        let upcoming = session.timeline.take_upcoming();
        if upcoming.is_some() {
            session.feed.decoder = (self.open)(&audible.path)?;
        }
        let landed_ms = session.feed.decoder.seek(position_ms)?;
        session.feed.rewind(frame);
        session.timeline.reanchor(frame, landed_ms);
        self.next_position_event = Instant::now();

        // A newer queue entry wins over the one that was decoded ahead.
        if let Some(track) = upcoming.filter(|_| keep_upcoming && self.next.is_none()) {
            match Queued::open(self.open, track.id, track.path) {
                Ok(queued) => self.next = Some(queued),
                Err(error) => self.report(Some(track.id), &error),
            }
        }
        Ok(())
    }

    fn tick(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Err(error) = session.fill(&mut self.next) {
            let feeding = session.feeding_id();
            self.fail(Some(feeding), error);
            return;
        }

        let consumed = session.output.consumed();
        if let Some(transition) = session.timeline.advance(consumed) {
            (self.emit)(PlayerEvent::TrackEnded {
                track_id: transition.ended,
            });
            (self.emit)(PlayerEvent::TrackStarted {
                track_id: transition.started.id,
            });
        }
        if session.finished() {
            self.finish_track();
            return;
        }

        self.publish();
        if self.state == PlaybackState::Playing && Instant::now() >= self.next_position_event {
            let status = self.status();
            if let Some(track_id) = status.track_id {
                (self.emit)(PlayerEvent::Position {
                    track_id,
                    position_ms: status.position_ms,
                    duration_ms: status.duration_ms,
                });
            }
            self.next_position_event = Instant::now() + POSITION_INTERVAL;
        }
    }

    /// The last queued frame has been played: move to the next track or stop.
    fn finish_track(&mut self) {
        let Some(ended) = self.audible_id() else {
            return;
        };
        (self.emit)(PlayerEvent::TrackEnded { track_id: ended });
        match self.next.take() {
            // Only reached for a format the open stream cannot carry; `start` reopens the device.
            Some(queued) => {
                let id = queued.track.id;
                if let Err(error) = self.start(queued) {
                    self.fail(Some(id), error);
                }
            }
            None => {
                self.release();
                self.state = PlaybackState::Stopped;
                self.announce();
                self.restore_device();
            }
        }
    }

    fn audible_id(&self) -> Option<i64> {
        self.session
            .as_ref()
            .map(|session| session.timeline.current().id)
    }

    /// Fades out and closes the stream, giving the device time to play out its buffer.
    fn release(&mut self) {
        if let Some(session) = self.session.take() {
            // A failed flush means the callback is gone, so there is nothing left to fade.
            if session.output.flush().is_ok() {
                thread::sleep(DRAIN_GRACE);
            }
        }
    }

    fn fail(&mut self, track_id: Option<i64>, error: Error) {
        // A dead stream cannot fade; with a healthy one the listener should hear a fade, not a cut.
        if matches!(error, Error::Output(_) | Error::OutputStalled) {
            self.session = None;
        } else {
            self.release();
        }
        self.next = None;
        self.state = PlaybackState::Stopped;
        self.report(track_id, &error);
        self.announce();
        self.restore_device();
    }

    /// Puts the output device back as it was before playback changed it.
    fn restore_device(&mut self) {
        if let Some(saved) = self.saved_device.take() {
            saved.restore();
        }
    }

    fn report(&self, track_id: Option<i64>, error: &Error) {
        (self.emit)(PlayerEvent::Error {
            track_id,
            message: error.to_string(),
        });
    }

    fn status(&self) -> PlaybackStatus {
        let volume = self.volume.get();
        let Some(session) = &self.session else {
            return PlaybackStatus::stopped(volume);
        };
        let track = session.timeline.current();
        PlaybackStatus {
            state: self.state,
            track_id: Some(track.id),
            position_ms: session.timeline.position_ms(session.output.consumed()),
            duration_ms: track.duration_ms,
            sample_rate: Some(track.sample_rate),
            output_sample_rate: Some(session.output.format().sample_rate),
            volume,
        }
    }

    fn publish(&self) {
        *self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = self.status();
    }

    /// Publishes the status and tells listeners the state changed.
    fn announce(&self) {
        self.publish();
        (self.emit)(PlayerEvent::StateChanged(self.status()));
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.release();
        self.restore_device();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::mpsc;

    use super::*;
    use crate::decoder::fault::{self, Fault, Faulty};

    const STEREO_44K: Format = Format {
        sample_rate: 44_100,
        channels: 2,
    };
    const STEREO_48K: Format = Format {
        sample_rate: 48_000,
        channels: 2,
    };

    /// Stands in for the ring and the device: bounded queue in, one frame at a time out.
    struct Device {
        queue: VecDeque<f32>,
        capacity: usize,
        channels: usize,
    }

    impl Device {
        fn new(capacity_frames: usize, format: Format) -> Self {
            let channels = usize::from(format.channels);
            Device {
                queue: VecDeque::new(),
                capacity: capacity_frames * channels,
                channels,
            }
        }
    }

    impl Sink for Device {
        fn push(&mut self, samples: &[f32]) -> usize {
            let count = (self.capacity - self.queue.len()).min(samples.len()) / self.channels
                * self.channels;
            self.queue.extend(&samples[..count]);
            count
        }
    }

    struct Heard {
        samples: Vec<f32>,
        /// `(ended, started, consumed frames when the transition fired)`.
        transitions: Vec<(i64, i64, u64)>,
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn queued(id: i64, name: &str) -> Queued {
        Queued::open(Decoder::open, id, fixture(name)).unwrap()
    }

    fn decode(name: &str) -> Vec<f32> {
        let mut decoder = Decoder::open(&fixture(name)).unwrap();
        let mut samples = Vec::new();
        while let Some(chunk) = decoder.next_chunk().unwrap() {
            samples.extend_from_slice(chunk);
        }
        samples
    }

    fn start(first: &str, format: Format) -> (Feed, Timeline) {
        let Queued { track, decoder } = queued(1, first);
        (
            Feed::new(decoder, format, 0).unwrap(),
            Timeline::new(format.sample_rate, track, 0, 0),
        )
    }

    /// Plays everything the feed can produce, advancing the timeline after every single frame so
    /// a transition is observed on exactly the frame it takes effect.
    fn play_out(
        feed: &mut Feed,
        timeline: &mut Timeline,
        next: &mut Option<Queued>,
        device: &mut Device,
    ) -> Heard {
        let mut heard = Heard {
            samples: Vec::new(),
            transitions: Vec::new(),
        };
        let mut consumed = 0;
        loop {
            feed.fill(device, timeline, next).unwrap();
            if device.queue.is_empty() {
                return heard;
            }
            heard.samples.extend(device.queue.drain(..device.channels));
            consumed += 1;
            if let Some(transition) = timeline.advance(consumed) {
                heard
                    .transitions
                    .push((transition.ended, transition.started.id, consumed));
            }
        }
    }

    #[test]
    fn gapless_handoff_is_sample_exact_and_lands_on_the_boundary_frame() {
        let (mut feed, mut timeline) = start("sine.flac", STEREO_44K);
        let mut next = Some(queued(2, "sine.mp3"));
        let mut device = Device::new(4_096, STEREO_44K);

        let heard = play_out(&mut feed, &mut timeline, &mut next, &mut device);

        let expected = [decode("sine.flac"), decode("sine.mp3")].concat();
        assert!(
            heard.samples == expected,
            "stream is the two tracks back to back, nothing between"
        );
        assert_eq!(heard.transitions, [(1, 2, 44_100)]);
        assert!(next.is_none());
        assert_eq!(timeline.current().id, 2);
        assert_eq!(timeline.position_ms(88_200), 1_000);
    }

    #[test]
    fn a_ring_deeper_than_a_track_still_hands_off_one_track_at_a_time() {
        let (mut feed, mut timeline) = start("sine.flac", STEREO_44K);
        let mut next = Some(queued(2, "sine.mp3"));
        let mut device = Device::new(500_000, STEREO_44K);
        // Both tracks fit, so the second is fully decoded before the first has been heard.
        feed.fill(&mut device, &mut timeline, &mut next).unwrap();
        assert_eq!(timeline.upcoming().map(|track| track.id), Some(2));

        next = Some(queued(3, "sine.ogg"));
        feed.fill(&mut device, &mut timeline, &mut next).unwrap();
        assert!(
            next.is_some(),
            "third track waits until the second has started"
        );

        let heard = play_out(&mut feed, &mut timeline, &mut next, &mut device);

        let expected = [decode("sine.flac"), decode("sine.mp3"), decode("sine.ogg")].concat();
        assert!(heard.samples == expected);
        assert_eq!(heard.transitions, [(1, 2, 44_100), (2, 3, 88_200)]);
    }

    #[test]
    fn a_different_format_is_left_queued_for_the_engine_to_reopen_the_device() {
        let (mut feed, mut timeline) = start("sine.flac", STEREO_44K);
        let mut next = Some(queued(2, "sine-alac.m4a"));
        let mut device = Device::new(4_096, STEREO_44K);

        let heard = play_out(&mut feed, &mut timeline, &mut next, &mut device);

        assert!(heard.samples == decode("sine.flac"));
        assert!(heard.transitions.is_empty());
        assert!(feed.eof && next.is_some() && timeline.upcoming().is_none());
    }

    #[test]
    fn handoff_through_the_resampler_lands_on_the_converted_boundary() {
        let (mut feed, mut timeline) = start("sine-22k.aiff", STEREO_48K);
        let mut next = Some(queued(2, "sine-22k.aiff"));
        let mut device = Device::new(4_096, STEREO_48K);

        let heard = play_out(&mut feed, &mut timeline, &mut next, &mut device);

        // One second of 22.05 kHz audio becomes exactly one second at 48 kHz, per track.
        assert_eq!(heard.samples.len(), 2 * 2 * 48_000);
        assert_eq!(heard.transitions, [(1, 2, 48_000)]);
    }

    #[test]
    fn a_track_queued_after_the_end_was_decoded_is_still_picked_up() {
        let (mut feed, mut timeline) = start("sine.flac", STEREO_44K);
        let mut next = None;
        let mut device = Device::new(500_000, STEREO_44K);
        feed.fill(&mut device, &mut timeline, &mut next).unwrap();
        assert!(feed.eof);

        next = Some(queued(2, "sine.mp3"));
        let heard = play_out(&mut feed, &mut timeline, &mut next, &mut device);

        assert_eq!(heard.transitions, [(1, 2, 44_100)]);
    }

    #[test]
    fn rewind_discards_staged_audio_and_restarts_the_stream_index() {
        let (mut feed, mut timeline) = start("sine.flac", STEREO_44K);
        let mut next = None;
        let mut device = Device::new(1_024, STEREO_44K);
        feed.fill(&mut device, &mut timeline, &mut next).unwrap();
        assert!(feed.produced > 1_024, "decoded ahead of the device");

        device.queue.clear();
        feed.rewind(5_000);
        assert_eq!(
            (feed.produced, feed.is_staging(), feed.eof),
            (5_000, false, false)
        );
    }

    #[test]
    fn a_decoder_panic_mid_stream_is_an_error_from_fill() {
        let decoder =
            Decoder::open_with(&fixture("sine.flac"), Faulty::wrap(Fault::NextPacket)).unwrap();
        let mut feed = Feed::new(decoder, STEREO_44K, 0).unwrap();
        let (_, mut timeline) = start("sine.flac", STEREO_44K);
        let mut device = Device::new(4_096, STEREO_44K);

        let outcome = feed.fill(&mut device, &mut timeline, &mut None);

        assert!(matches!(outcome, Err(Error::Panicked(_))));
        // The thread that hit the panic is still here to carry on with the next track.
        let (mut next_feed, mut next_timeline) = start("sine.mp3", STEREO_44K);
        next_feed
            .fill(&mut device, &mut next_timeline, &mut None)
            .unwrap();
        assert!(!device.queue.is_empty());
    }

    struct Harness {
        engine: Engine,
        events: Arc<Mutex<Vec<PlayerEvent>>>,
    }

    impl Harness {
        fn new(open: Opener) -> Self {
            let (sender, commands) = mpsc::channel();
            let events = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&events);
            let emit = Arc::new(move |event| sink.lock().unwrap().push(event));
            let status = Arc::new(Mutex::new(PlaybackStatus::stopped(0.0)));
            let mut engine =
                Engine::new(commands, sender, emit, Arc::new(Volume::new(0.0)), status);
            engine.open = open;
            Harness { engine, events }
        }

        fn events(&self) -> Vec<PlayerEvent> {
            std::mem::take(&mut *self.events.lock().unwrap())
        }
    }

    #[test]
    fn a_panic_opening_the_track_to_play_stops_with_an_error_and_the_engine_carries_on() {
        let mut harness = Harness::new(fault::open_panics);

        assert!(
            harness
                .engine
                .handle(Command::Play(7, fixture("sine.flac")))
                .is_continue()
        );

        let events = harness.events();
        assert!(
            matches!(&events[0], PlayerEvent::Error { track_id: Some(7), message } if message.contains("crashed")),
            "{events:?}"
        );
        assert!(
            matches!(&events[1], PlayerEvent::StateChanged(status)
                if status.state == PlaybackState::Stopped && status.track_id.is_none()),
            "{events:?}"
        );

        // The next command is handled normally.
        harness.engine.open = Decoder::open;
        assert!(
            harness
                .engine
                .handle(Command::Play(8, fixture("missing.flac")))
                .is_continue()
        );
        let events = harness.events();
        assert!(
            matches!(
                &events[0],
                PlayerEvent::Error {
                    track_id: Some(8),
                    ..
                }
            ),
            "{events:?}"
        );
        assert!(harness.engine.handle(Command::Shutdown).is_break());
    }

    #[test]
    fn a_panic_opening_the_queued_track_is_reported_and_nothing_is_queued() {
        let mut harness = Harness::new(fault::open_panics);

        assert!(
            harness
                .engine
                .handle(Command::QueueNext(9, fixture("sine.flac")))
                .is_continue()
        );

        let events = harness.events();
        assert!(
            matches!(&events[0], PlayerEvent::Error { track_id: Some(9), message } if message.contains("crashed")),
            "{events:?}"
        );
        assert!(harness.engine.next.is_none());
    }

    #[test]
    fn gapless_aac_albums_join_without_priming_or_padding() {
        // Two m4a files from different encoders: Apple's iTunSMPB tag and an ffmpeg edit list.
        let (mut feed, mut timeline) = start("gapless-itunes.m4a", STEREO_44K);
        let mut next = Some(queued(2, "gapless-ffmpeg.m4a"));
        let mut device = Device::new(4_096, STEREO_44K);

        let heard = play_out(&mut feed, &mut timeline, &mut next, &mut device);

        assert_eq!(heard.samples.len(), 2 * 2 * 43_210);
        assert_eq!(heard.transitions, [(1, 2, 43_210)]);
    }
}
