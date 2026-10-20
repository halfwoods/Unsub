use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::decoder::Decoder;
use crate::gain::Gain;
use crate::host::{DeviceState, Host};
use crate::mix::{ChannelLayout, Mixer};
use crate::output::{Format, Output, Request, Sink};
use crate::player::{PlaybackState, PlaybackStatus, PlayerEvent, Shared};
use crate::resample::Resampler;
use crate::timeline::{Timeline, TrackInfo};
use crate::{Error, Result};

/// How often the engine tops up the ring, notices track boundaries and refreshes the status.
const TICK: Duration = Duration::from_millis(10);

const POSITION_INTERVAL: Duration = Duration::from_millis(250);

/// The device still holds up to a buffer of audio the callback has already handed over.
const DRAIN_GRACE: Duration = Duration::from_millis(50);

pub(crate) enum Command {
    Play(i64, PathBuf, Option<Gain>),
    QueueNext(i64, PathBuf, Option<Gain>),
    ClearNext,
    Pause,
    Resume,
    Stop,
    Seek(u64),
    SetBitPerfect(bool),
    /// The volume was set while the device volume is in charge.
    VolumeIgnored,
    /// Sent by the stream's error callback, which must not touch the stream itself.
    OutputFailed {
        generation: u64,
        error: Error,
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
    fn open(open: Opener, id: i64, path: PathBuf, gain: Option<Gain>) -> Result<Queued> {
        let decoder = open(&path)?;
        let track = TrackInfo {
            id,
            path,
            duration_ms: decoder.duration_ms().unwrap_or(0),
            sample_rate: decoder.sample_rate(),
            gain,
        };
        Ok(Queued { track, decoder })
    }
}

/// Decoder output on its way to the ring: converted to the device format and staged until the
/// ring has room.
struct Feed {
    decoder: Decoder,
    source: Format,
    layout: ChannelLayout,
    /// In bit-perfect mode a change of bit depth needs a different device format.
    depth: Option<u32>,
    exact: bool,
    /// Linear scale of the track being decoded. It changes with the track, so a gapless
    /// boundary is also a change of gain on exactly the boundary frame.
    amplitude: f32,
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
    fn new(
        decoder: Decoder,
        output: Format,
        produced: u64,
        exact: bool,
        gain: Option<Gain>,
    ) -> Result<Feed> {
        let source = Format {
            sample_rate: decoder.sample_rate(),
            channels: decoder.channels(),
        };
        let resampler = (source != output)
            .then(|| {
                let mixer = Mixer::new(decoder.channel_layout(), output.channels);
                Resampler::new(source.sample_rate, output.sample_rate, mixer)
            })
            .transpose()?;
        Ok(Feed {
            layout: decoder.channel_layout(),
            depth: decoder.bit_depth(),
            exact,
            amplitude: amplitude(gain, exact),
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
            && decoder.channel_layout() == self.layout
            && (!self.exact || decoder.bit_depth() == self.depth)
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
        if self.amplitude != 1.0 {
            for sample in &mut self.staging[before..] {
                *sample *= self.amplitude;
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
            let gain = queued.track.gain;
            timeline.schedule(self.produced, queued.track);
            self.continue_with(queued.decoder, gain);
        }
    }

    /// Continues with the next track straight after the current one's last frame.
    fn continue_with(&mut self, decoder: Decoder, gain: Option<Gain>) {
        self.decoder = decoder;
        self.amplitude = amplitude(gain, self.exact);
        self.eof = false;
    }

    /// Discards everything decoded so far; the ring must already be empty at stream frame `frame`.
    /// `gain` is the one of the track the decoder is now on.
    fn rewind(&mut self, frame: u64, gain: Option<Gain>) {
        self.amplitude = amplitude(gain, self.exact);
        self.staging.clear();
        self.staged = 0;
        self.produced = frame;
        self.eof = false;
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
    }
}

/// The scale Sound Check's `gain` makes to a track's samples. A bit-perfect stream carries the
/// file's samples untouched, so there it is none.
fn amplitude(gain: Option<Gain>, exact: bool) -> f32 {
    match gain {
        Some(gain) if !exact => gain.amplitude(),
        _ => 1.0,
    }
}

/// One open output stream and the track timeline playing through it.
struct Session {
    output: Output,
    feed: Feed,
    timeline: Timeline,
}

impl Session {
    /// Starts `queued`, whose decoder is already at `position_ms`, on a freshly opened `output`.
    fn new(output: Output, queued: Queued, position_ms: u64) -> Result<Session> {
        let Queued { track, decoder } = queued;
        let format = output.format();
        let exact = output.request().exact;
        Ok(Session {
            feed: Feed::new(decoder, format, 0, exact, track.gain)?,
            timeline: Timeline::new(format.sample_rate, track, 0, position_ms),
            output,
        })
    }

    /// Whether this output can carry `decoder` as it is, with no reopening.
    fn fits(&self, decoder: &Decoder, exact: bool) -> bool {
        self.output.request().exact == exact && self.feed.accepts(decoder)
    }

    /// Decodes into the ring until it is full, handing over to `next` when this track ends.
    fn fill(&mut self, next: &mut Option<Queued>) -> Result<()> {
        self.feed.fill(&mut self.output, &mut self.timeline, next)
    }

    /// Restarts on a new track, already at `position_ms`, after the output has been flushed.
    fn restart(&mut self, queued: Queued, position_ms: u64) {
        let frame = self.output.consumed();
        self.feed.decoder = queued.decoder;
        self.feed.rewind(frame, queued.track.gain);
        self.timeline = Timeline::new(
            self.output.format().sample_rate,
            queued.track,
            frame,
            position_ms,
        );
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
    shared: Arc<Shared>,
    host: Box<dyn Host>,
    open: Opener,
    /// The output device as it was before this engine first changed it, restored once playback
    /// stops. Kept across format changes within one run of playback.
    device: Option<Box<dyn DeviceState>>,
    /// Whether bit-perfect output is wanted; whether the open stream achieves it is in its info.
    bit_perfect: bool,
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
        shared: Arc<Shared>,
        host: Box<dyn Host>,
    ) -> Self {
        Engine {
            commands,
            sender,
            emit,
            shared,
            host,
            open: Decoder::open,
            device: None,
            bit_perfect: false,
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
            Command::Play(id, path, gain) => self.play(id, path, gain),
            Command::QueueNext(id, path, gain) => self.queue_next(id, path, gain),
            Command::ClearNext => self.clear_next(),
            Command::Pause => self.set_paused(true),
            Command::Resume => self.set_paused(false),
            Command::Stop => self.stop(),
            Command::Seek(position_ms) => self.seek(position_ms),
            Command::SetBitPerfect(wanted) => self.set_bit_perfect(wanted),
            Command::VolumeIgnored => self.volume_ignored(),
            Command::OutputFailed { generation, error } if generation == self.generation => {
                self.fail(self.audible_id(), error);
            }
            Command::OutputFailed { .. } => {}
            Command::Shutdown => return ControlFlow::Break(()),
        }
        ControlFlow::Continue(())
    }

    fn play(&mut self, id: i64, path: PathBuf, gain: Option<Gain>) {
        self.next = None;
        let started = Queued::open(self.open, id, path, gain).and_then(|queued| self.start(queued));
        if let Err(error) = started {
            self.fail(Some(id), error);
        }
    }

    /// Starts `queued` from its beginning, as a new track.
    fn start(&mut self, queued: Queued) -> Result<()> {
        let id = queued.track.id;
        self.begin(queued, 0)?;
        self.state = PlaybackState::Playing;
        self.next_position_event = Instant::now();
        (self.emit)(PlayerEvent::TrackStarted { track_id: id });
        self.announce();
        Ok(())
    }

    /// Points playback at `queued`, whose decoder is already at `position_ms`. The open output
    /// is kept when it fits, so there is no reopen pop.
    fn begin(&mut self, queued: Queued, position_ms: u64) -> Result<()> {
        let exact = self.bit_perfect;
        match self
            .session
            .as_mut()
            .filter(|session| session.fits(&queued.decoder, exact))
        {
            Some(session) => {
                session.output.flush()?;
                session.restart(queued, position_ms);
                session
                    .output
                    .set_paused(self.state == PlaybackState::Paused);
                session.fill(&mut self.next)?;
            }
            None => {
                self.release();
                let output = self.open_output(&queued.decoder)?;
                let mut session = Session::new(output, queued, position_ms)?;
                session
                    .output
                    .set_paused(self.state == PlaybackState::Paused);
                session.fill(&mut self.next)?;
                // The ring is primed first so the first callback already has audio.
                session.output.start()?;
                self.session = Some(session);
            }
        }
        Ok(())
    }

    /// Opens an output for `decoder`, bit-perfect if wanted and the device allows, and says so
    /// when it does not.
    fn open_output(&mut self, decoder: &Decoder) -> Result<Output> {
        if self.device.is_none() {
            self.device = self.host.capture();
        }
        let request = Request {
            source: Format {
                sample_rate: decoder.sample_rate(),
                channels: decoder.channels(),
            },
            bit_depth: decoder.bit_depth(),
            exact: self.bit_perfect,
        };
        if request.exact {
            match self.open_exact(&request) {
                Err(Error::BitPerfectUnavailable(reason)) => self.notice(format!(
                    "Bit-perfect playback is not possible for this track: {reason}."
                )),
                other => return other,
            }
        }
        if let Some(device) = self.device.as_mut() {
            device.release_exclusive();
        }
        self.open_stream(&Request {
            exact: false,
            ..request
        })
    }

    fn open_exact(&mut self, request: &Request) -> Result<Output> {
        let exclusive = self
            .device
            .as_mut()
            .is_some_and(|device| device.take_exclusive());
        if !exclusive {
            return Err(Error::BitPerfectUnavailable(
                "another app is using the device, or exclusive access is not available here"
                    .to_owned(),
            ));
        }
        self.open_stream(request)
    }

    fn open_stream(&mut self, request: &Request) -> Result<Output> {
        self.generation += 1;
        let (sender, generation) = (self.sender.clone(), self.generation);
        let on_failure = move |error| {
            let _ = sender.send(Command::OutputFailed { generation, error });
        };
        self.host.open(
            request,
            Arc::clone(&self.shared.volume),
            Box::new(on_failure),
        )
    }

    fn queue_next(&mut self, id: i64, path: PathBuf, gain: Option<Gain>) {
        match Queued::open(self.open, id, path, gain) {
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

    fn set_bit_perfect(&mut self, wanted: bool) {
        if self.bit_perfect == wanted {
            return;
        }
        self.bit_perfect = wanted;
        // With nothing playing the choice simply applies to the next stream.
        if self.session.is_some()
            && let Err(error) = self.reopen_output()
        {
            self.fail(self.audible_id(), error);
        }
        self.announce();
    }

    /// Moves the audible track to a new output, for a change of mode, resuming where it was.
    fn reopen_output(&mut self) -> Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        session.output.flush()?;
        let audible = session.timeline.current().clone();
        let position_ms = session.timeline.position_ms(session.output.consumed());
        let position_ms = match audible.duration_ms {
            0 => position_ms,
            duration => position_ms.min(duration.saturating_sub(1)),
        };
        let upcoming = session.timeline.take_upcoming();

        let mut queued = Queued::open(self.open, audible.id, audible.path, audible.gain)?;
        let landed_ms = queued.decoder.seek(position_ms)?;
        self.release();
        if let Some(track) = upcoming.filter(|_| self.next.is_none()) {
            match Queued::open(self.open, track.id, track.path, track.gain) {
                Ok(requeued) => self.next = Some(requeued),
                Err(error) => self.report(Some(track.id), &error),
            }
        }
        self.begin(queued, landed_ms)
    }

    fn volume_ignored(&mut self) {
        let locked = self
            .session
            .as_ref()
            .is_some_and(|session| session.output.info().bit_perfect);
        if locked {
            self.notice(
                "Bit-perfect mode plays at full scale; use the device's own volume control."
                    .to_owned(),
            );
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
        // With a handoff pending the feed had moved on to the next track's gain.
        session.feed.rewind(frame, audible.gain);
        session.timeline.reanchor(frame, landed_ms);
        self.next_position_event = Instant::now();

        // A newer queue entry wins over the one that was decoded ahead.
        if let Some(track) = upcoming.filter(|_| keep_upcoming && self.next.is_none()) {
            match Queued::open(self.open, track.id, track.path, track.gain) {
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
        if matches!(
            error,
            Error::Output(_) | Error::OutputStalled | Error::Device(_)
        ) {
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
        if let Some(device) = self.device.take() {
            device.restore();
        }
    }

    fn report(&self, track_id: Option<i64>, error: &Error) {
        (self.emit)(PlayerEvent::Error {
            track_id,
            message: error.to_string(),
        });
    }

    fn notice(&self, message: String) {
        (self.emit)(PlayerEvent::Notice { message });
    }

    fn status(&self) -> PlaybackStatus {
        let Some(session) = &self.session else {
            return PlaybackStatus::stopped(self.shared.volume.get());
        };
        let track = session.timeline.current();
        let info = session.output.info();
        PlaybackStatus {
            state: self.state,
            track_id: Some(track.id),
            position_ms: session.timeline.position_ms(session.output.consumed()),
            duration_ms: track.duration_ms,
            sample_rate: Some(track.sample_rate),
            output_sample_rate: Some(session.output.format().sample_rate),
            volume: if info.bit_perfect {
                1.0
            } else {
                self.shared.volume.get()
            },
            bit_perfect: info.bit_perfect,
            output_format: Some(info.label.clone()),
            gain_db: track
                .gain
                .filter(|_| !info.bit_perfect)
                .map(Gain::applied_db),
        }
    }

    fn publish(&self) {
        let status = self.status();
        self.shared
            .bit_perfect
            .store(status.bit_perfect, Ordering::Relaxed);
        *self
            .shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = status;
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
    use std::sync::atomic::AtomicBool;
    use std::sync::{Mutex, mpsc};

    use super::*;
    use crate::symphonia_source::fault::{self, Fault, Faulty};

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
        queued_with(id, name, None)
    }

    fn queued_with(id: i64, name: &str, gain: Option<Gain>) -> Queued {
        Queued::open(Decoder::open, id, fixture(name), gain).unwrap()
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
        start_with(first, format, false, None)
    }

    fn start_with(
        first: &str,
        format: Format,
        exact: bool,
        gain: Option<Gain>,
    ) -> (Feed, Timeline) {
        let Queued { track, decoder } = queued_with(1, first, gain);
        (
            Feed::new(decoder, format, 0, exact, gain).unwrap(),
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
        feed.rewind(5_000, None);
        assert_eq!(
            (feed.produced, feed.is_staging(), feed.eof),
            (5_000, false, false)
        );
    }

    #[test]
    fn a_decoder_panic_mid_stream_is_an_error_from_fill() {
        let decoder =
            Decoder::open_with(&fixture("sine.flac"), Faulty::wrap(Fault::NextPacket)).unwrap();
        let mut feed = Feed::new(decoder, STEREO_44K, 0, false, None).unwrap();
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
            let volume = Arc::new(crate::gain::Volume::new(0.0));
            let shared = Arc::new(Shared {
                status: Mutex::new(PlaybackStatus::stopped(0.0)),
                volume,
                bit_perfect: AtomicBool::new(false),
            });
            let (host, _) = crate::host::simulated::Simulated::new(Default::default());
            let mut engine = Engine::new(commands, sender, emit, shared, Box::new(host));
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
                .handle(Command::Play(7, fixture("sine.flac"), None))
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
                .handle(Command::Play(8, fixture("missing.flac"), None))
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
                .handle(Command::QueueNext(9, fixture("sine.flac"), None))
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

    #[test]
    fn a_surround_source_reaches_a_stereo_device_downmixed() {
        let (mut feed, mut timeline) = start("surround-5.1.flac", STEREO_44K);
        let mut device = Device::new(4_096, STEREO_44K);

        let heard = play_out(&mut feed, &mut timeline, &mut None, &mut device);

        assert_eq!(heard.samples.len(), 2 * 882, "20 ms of stereo frames");
        // Channels are 0.05 0.10 0.15 0.20 0.25 0.30: L R C LFE Ls Rs.
        let c = std::f32::consts::FRAC_1_SQRT_2;
        let gain = 1.0 / (1.0 + 2.0 * c);
        let (left, right) = (heard.samples[800], heard.samples[801]);
        let (want_left, want_right) = (
            (0.05 + c * 0.15 + c * 0.25) * gain,
            (0.10 + c * 0.15 + c * 0.30) * gain,
        );
        assert!((left - want_left).abs() < 1e-3 && (right - want_right).abs() < 1e-3);
    }

    /// The fixtures are 440 Hz sines peaking at 0.5 (-6.02 dBFS).
    const SINE_PEAK_DBTP: f32 = -6.02;

    fn gain(db: f32) -> Gain {
        Gain {
            db,
            peak_dbtp: SINE_PEAK_DBTP,
        }
    }

    fn scaled(samples: &[f32], gain: Option<Gain>) -> Vec<f32> {
        let amplitude = gain.map_or(1.0, Gain::amplitude);
        samples.iter().map(|sample| sample * amplitude).collect()
    }

    #[test]
    fn each_track_is_scaled_by_its_own_gain_and_the_change_lands_on_the_boundary_frame() {
        let (quiet, loud) = (gain(-6.0), gain(3.0));
        let (mut feed, mut timeline) = start_with("sine.flac", STEREO_44K, false, Some(quiet));
        let mut next = Some(queued_with(2, "sine.mp3", Some(loud)));
        let mut device = Device::new(4_096, STEREO_44K);

        let heard = play_out(&mut feed, &mut timeline, &mut next, &mut device);

        let expected = [
            scaled(&decode("sine.flac"), Some(quiet)),
            scaled(&decode("sine.mp3"), Some(loud)),
        ]
        .concat();
        assert!(
            heard.samples == expected,
            "the first sample of the second track is the first one at its gain"
        );
        assert_eq!(heard.transitions, [(1, 2, 44_100)]);
    }

    #[test]
    fn a_track_without_gain_after_one_with_it_returns_to_unity() {
        let boosted = gain(4.0);
        let (mut feed, mut timeline) = start_with("sine.flac", STEREO_44K, false, Some(boosted));
        let mut next = Some(queued(2, "sine.mp3"));
        let mut device = Device::new(500_000, STEREO_44K);

        let heard = play_out(&mut feed, &mut timeline, &mut next, &mut device);

        let expected = [
            scaled(&decode("sine.flac"), Some(boosted)),
            decode("sine.mp3"),
        ]
        .concat();
        assert!(heard.samples == expected);
    }

    #[test]
    fn the_gain_changes_on_the_converted_boundary_frame_when_resampling() {
        let (mut plain_feed, mut plain_timeline) = start("sine-22k.aiff", STEREO_48K);
        let mut next = Some(queued(2, "sine-22k.aiff"));
        let mut device = Device::new(4_096, STEREO_48K);
        let plain = play_out(&mut plain_feed, &mut plain_timeline, &mut next, &mut device);

        let (quiet, loud) = (gain(-9.0), gain(2.0));
        let (mut feed, mut timeline) = start_with("sine-22k.aiff", STEREO_48K, false, Some(quiet));
        let mut next = Some(queued_with(2, "sine-22k.aiff", Some(loud)));
        let heard = play_out(&mut feed, &mut timeline, &mut next, &mut device);

        // The resampler's last frames of the first track belong to it, not to the next.
        let boundary = 2 * 48_000;
        let expected = [
            scaled(&plain.samples[..boundary], Some(quiet)),
            scaled(&plain.samples[boundary..], Some(loud)),
        ]
        .concat();
        assert!(heard.samples == expected);
        assert_eq!(heard.transitions, [(1, 2, 48_000)]);
    }

    #[test]
    fn a_boost_is_held_back_so_the_peak_ends_at_minus_one_dbtp() {
        let (mut feed, mut timeline) = start_with("sine.flac", STEREO_44K, false, Some(gain(12.0)));
        let mut device = Device::new(4_096, STEREO_44K);

        let heard = play_out(&mut feed, &mut timeline, &mut None, &mut device);

        let peak = |samples: &[f32]| samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        let before = peak(&decode("sine.flac"));
        let after = peak(&heard.samples);
        let wanted = 10f32.powf(-1.0 / 20.0);
        assert!(
            (after - wanted).abs() < 1e-3,
            "peak {after}, wanted {wanted}"
        );
        assert!(
            (after / before - 10f32.powf((-1.0 - SINE_PEAK_DBTP) / 20.0)).abs() < 1e-3,
            "a +12 dB request became +{} dB",
            -1.0 - SINE_PEAK_DBTP
        );
    }

    #[test]
    fn a_bit_perfect_feed_leaves_the_samples_alone() {
        let (mut feed, mut timeline) = start_with("sine.flac", STEREO_44K, true, Some(gain(-6.0)));
        // A bit-perfect stream only continues into a track of the same bit depth.
        let mut next = Some(queued_with(2, "sine.flac", Some(gain(3.0))));
        let mut device = Device::new(4_096, STEREO_44K);

        let heard = play_out(&mut feed, &mut timeline, &mut next, &mut device);

        let expected = [decode("sine.flac"), decode("sine.flac")].concat();
        assert!(heard.samples == expected);
        assert_eq!(heard.transitions, [(1, 2, 44_100)]);
    }

    #[test]
    fn rewinding_puts_back_the_gain_of_the_track_being_replayed() {
        let (quiet, loud) = (gain(-6.0), gain(3.0));
        let (mut feed, mut timeline) = start_with("sine.flac", STEREO_44K, false, Some(quiet));
        let mut next = Some(queued_with(2, "sine.mp3", Some(loud)));
        let mut device = Device::new(500_000, STEREO_44K);
        feed.fill(&mut device, &mut timeline, &mut next).unwrap();
        assert_eq!(timeline.upcoming().map(|track| track.id), Some(2));

        // What `Engine::reposition` does for a seek with a handoff pending.
        device.queue.clear();
        feed.decoder = Decoder::open(&fixture("sine.flac")).unwrap();
        feed.rewind(0, timeline.current().gain);
        timeline.take_upcoming();
        feed.fill(&mut device, &mut timeline, &mut None).unwrap();

        let replayed: Vec<f32> = device.queue.drain(..).collect();
        assert!(replayed == scaled(&decode("sine.flac"), Some(quiet)));
    }

    #[test]
    fn the_status_reports_the_applied_gain_of_the_audible_track_only_outside_bit_perfect() {
        let mut harness = Harness::new(Decoder::open);
        let quiet = gain(-6.0);
        let play = Command::Play(1, fixture("sine.flac"), Some(quiet));
        assert!(harness.engine.handle(play).is_continue());
        assert_eq!(harness.engine.status().gain_db, Some(-6.0));

        assert!(
            harness
                .engine
                .handle(Command::SetBitPerfect(true))
                .is_continue()
        );
        assert_eq!(harness.engine.status().gain_db, None);
        assert!(
            harness
                .engine
                .handle(Command::SetBitPerfect(false))
                .is_continue()
        );
        assert_eq!(harness.engine.status().gain_db, Some(-6.0));
        assert!(harness.engine.handle(Command::Shutdown).is_break());
    }
}
