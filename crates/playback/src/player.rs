use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;

use crate::Result;
use crate::engine::{Command, Engine};
use crate::equalizer::{EqControl, Equalizer};
use crate::gain::Volume;
use crate::host::{Host, SystemHost};
use crate::live::LiveFormat;
use crate::tap::{Tap, TapHub};
use crate::timeline::TrackOptions;

const MIN_CROSSFADE_MS: u128 = 1_000;
const MAX_CROSSFADE_MS: u128 = 12_000;

/// How much of a live stream is decoded before it starts, unless asked otherwise.
const DEFAULT_LIVE_BUFFER_MS: u32 = 2_000;
const MIN_LIVE_BUFFER_MS: u128 = 250;
const MAX_LIVE_BUFFER_MS: u128 = 30_000;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackStatus {
    pub state: PlaybackState,
    pub track_id: Option<i64>,
    pub position_ms: u64,
    /// Zero when the container does not say how long the track is.
    pub duration_ms: u64,
    /// The file's rate.
    pub sample_rate: Option<u32>,
    /// The device's rate; differs from `sample_rate` when the device cannot run at the file's.
    pub output_sample_rate: Option<u32>,
    pub volume: f32,
    /// Whether the stream really is bit-perfect right now: exclusive access to the device, its
    /// rate and sample type matched to the file, and no volume applied. Asking for bit-perfect
    /// output does not make this true if the device cannot do it.
    pub bit_perfect: bool,
    /// What the device is being fed, e.g. "24-bit 96 kHz".
    pub output_format: Option<String>,
    /// The Sound Check gain being applied to the audible track, after it has been held back to
    /// keep the track from clipping. `None` when the track has none, and always in bit-perfect
    /// mode, which plays the file's samples untouched.
    pub gain_db: Option<f32>,
    /// Whether the equalizer is shaping the sound: one is set and the stream is not bit-perfect.
    pub eq_active: bool,
    /// Whether this is a live stream: it has no duration (`duration_ms` is zero), cannot seek,
    /// and `position_ms` is how long it has been heard.
    pub live: bool,
    /// Whether a live stream is waiting for enough audio to play, as it does for the first
    /// couple of seconds and after running dry. The state is still playing.
    pub buffering: bool,
}

impl PlaybackStatus {
    pub(crate) fn stopped(volume: f32) -> Self {
        PlaybackStatus {
            state: PlaybackState::Stopped,
            track_id: None,
            position_ms: 0,
            duration_ms: 0,
            sample_rate: None,
            output_sample_rate: None,
            volume,
            bit_perfect: false,
            output_format: None,
            gain_db: None,
            eq_active: false,
            live: false,
            buffering: false,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PlayerEvent {
    /// About four times a second while playing.
    Position {
        track_id: i64,
        position_ms: u64,
        duration_ms: u64,
    },
    StateChanged(PlaybackStatus),
    /// Also fires when a gapless transition reaches the next track.
    TrackStarted {
        track_id: i64,
    },
    /// The track played to its end; not sent when it is stopped or replaced.
    TrackEnded {
        track_id: i64,
    },
    Error {
        track_id: Option<i64>,
        message: String,
    },
    /// Something the user should be told that is not a failure, such as bit-perfect output
    /// being unavailable for a track, or the volume being ignored while it is on.
    Notice {
        message: String,
    },
    /// A live stream is waiting for audio: when it is started, and again each time it runs dry.
    /// Playback resumes by itself, announced by a [`PlayerEvent::StateChanged`] with
    /// `buffering` false.
    Buffering {
        id: i64,
    },
    /// A live stream is over, because the station closed it or it failed, and playback has
    /// stopped. Whether to connect again is up to the caller.
    LiveEnded {
        id: i64,
        reason: String,
    },
}

/// State the engine thread publishes and the [`Player`] reads.
pub(crate) struct Shared {
    /// Applied by the audio callback; ignored while a stream is bit-perfect.
    pub volume: Arc<Volume>,
    /// Applied by the audio callback; ignored while a stream is bit-perfect.
    pub equalizer: Arc<EqControl>,
    /// Copies of the output for any [`Tap`].
    pub tap: Arc<TapHub>,
    /// Crossfade length in milliseconds, zero for none.
    pub crossfade_ms: AtomicU32,
    /// How much of a live stream to decode before playing it, in milliseconds.
    pub live_buffer_ms: AtomicU32,
    pub status: Mutex<PlaybackStatus>,
    /// Whether the open stream is bit-perfect, so a volume change can say it is ignored.
    pub bit_perfect: AtomicBool,
}

/// Handle to the playback engine. Methods queue a command and return immediately.
pub struct Player {
    commands: Sender<Command>,
    shared: Arc<Shared>,
    engine: Option<JoinHandle<()>>,
}

impl Player {
    /// `on_event` runs on engine threads, never on the audio callback; keep it cheap, because a
    /// slow handler delays decoding.
    pub fn new(on_event: impl Fn(PlayerEvent) + Send + Sync + 'static) -> Result<Player> {
        Player::with_host(on_event, || Box::new(SystemHost::new()))
    }

    /// `new` with the audio system supplied, which tests use to play without a device. The host
    /// is built on the engine thread, which owns the audio streams.
    pub(crate) fn with_host(
        on_event: impl Fn(PlayerEvent) + Send + Sync + 'static,
        host: impl FnOnce() -> Box<dyn Host> + Send + 'static,
    ) -> Result<Player> {
        let (commands, receiver) = mpsc::channel();
        let volume = Arc::new(Volume::new(1.0));
        let shared = Arc::new(Shared {
            status: Mutex::new(PlaybackStatus::stopped(volume.get())),
            volume,
            equalizer: Arc::new(EqControl::default()),
            tap: Arc::default(),
            crossfade_ms: AtomicU32::new(0),
            live_buffer_ms: AtomicU32::new(DEFAULT_LIVE_BUFFER_MS),
            bit_perfect: AtomicBool::new(false),
        });

        let engine = {
            let (sender, shared) = (commands.clone(), Arc::clone(&shared));
            // The engine owns the audio stream, which is not `Send` on every platform, so it is
            // built on the thread that will use it.
            thread::Builder::new()
                .name("unsub-playback".into())
                .spawn(move || {
                    Engine::new(receiver, sender, Arc::new(on_event), shared, host()).run();
                })?
        };

        Ok(Player {
            commands,
            shared,
            engine: Some(engine),
        })
    }

    /// Replaces whatever is playing, and clears any queued next track.
    pub fn play(&self, track_id: i64, path: PathBuf) {
        self.play_with(track_id, path, None);
    }

    /// [`Player::play`] with a Sound Check adjustment for the track, which is not applied in
    /// bit-perfect mode, and the album it is from. A [`crate::Gain`] or an `Option<Gain>` will do for
    /// `options`.
    pub fn play_with(&self, track_id: i64, path: PathBuf, options: impl Into<TrackOptions>) {
        self.send(Command::Play(track_id, path, options.into()));
    }

    /// Plays a live stream, such as an internet radio station, replacing whatever is playing.
    /// `source` is read on a thread of its own, so it may block; `hint` says what it is (it may
    /// also be [`LiveFormat::Unknown`], to be worked out from the first bytes).
    ///
    /// A live stream is in a mode of its own:
    /// - There is no duration (`duration_ms` is zero, and `live` is set in the status), and
    ///   [`Player::seek`] does nothing. `position_ms` counts up from zero as it is heard.
    /// - Nothing is queued behind it, so [`Player::queue_next`] is ignored while it plays, and
    ///   nothing is crossfaded into it or out of it. Sound Check never applies to it, whatever
    ///   `options` says; there is no loudness to go by.
    /// - Playback starts once about two seconds are decoded (see [`Player::set_live_buffer`]),
    ///   announced by [`PlayerEvent::Buffering`], and holds again whenever the stream runs dry.
    /// - Pausing stops it: a live stream cannot be resumed where it was left, so [`Player::pause`]
    ///   discards the buffer and drops `source`, and listening again means calling `play_live`
    ///   with a new connection. [`Player::resume`] has nothing to resume.
    /// - When `source` ends or fails, what had been decoded plays out, and then playback stops
    ///   with [`PlayerEvent::LiveEnded`]; the caller decides whether to reconnect.
    /// - [`PlayerEvent::TrackStarted`] and [`PlayerEvent::TrackEnded`] are not sent, so that a
    ///   station is never counted as a play. Follow [`PlayerEvent::StateChanged`] instead.
    /// - The equalizer, the volume and bit-perfect mode apply as they do to files. A bit-perfect
    ///   stream whose format changes partway through reopens the device to match.
    ///
    /// Formats and bit rates may change partway through, as some stations do between segments or
    /// after reconnecting; the audio carries on, resampled if need be.
    pub fn play_live(
        &self,
        track_id: i64,
        source: Box<dyn Read + Send>,
        hint: LiveFormat,
        options: impl Into<TrackOptions>,
    ) {
        self.send(Command::PlayLive(track_id, source, hint, options.into()));
    }

    /// How much of a live stream is decoded before it plays, and again after it runs dry:
    /// between a quarter of a second and half a minute, two seconds unless set. More rides out
    /// longer stalls at the cost of a longer wait.
    pub fn set_live_buffer(&self, buffer: Duration) {
        let ms = buffer
            .as_millis()
            .clamp(MIN_LIVE_BUFFER_MS, MAX_LIVE_BUFFER_MS) as u32;
        self.shared.live_buffer_ms.store(ms, Ordering::Relaxed);
    }

    /// Plays `path` straight after the current track, without a gap when the format matches.
    /// Replaces any previously queued track.
    pub fn queue_next(&self, track_id: i64, path: PathBuf) {
        self.queue_next_with(track_id, path, None);
    }

    /// [`Player::queue_next`] with a Sound Check adjustment for the track, which takes effect on
    /// its first sample, whether it follows gaplessly or after a gap, and the album it is from,
    /// which decides whether it may be crossfaded in.
    pub fn queue_next_with(&self, track_id: i64, path: PathBuf, options: impl Into<TrackOptions>) {
        self.send(Command::QueueNext(track_id, path, options.into()));
    }

    pub fn clear_next(&self) {
        self.send(Command::ClearNext);
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn resume(&self) {
        self.send(Command::Resume);
    }

    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    pub fn seek(&self, position_ms: u64) {
        self.send(Command::Seek(position_ms));
    }

    /// `volume` is a slider position in `0.0..=1.0`; the loudness curve is applied internally.
    ///
    /// While a stream is bit-perfect the value is remembered but not applied, and a
    /// [`PlayerEvent::Notice`] says so.
    pub fn set_volume(&self, volume: f32) {
        self.shared.volume.set(volume);
        if self.shared.bit_perfect.load(Ordering::Relaxed) {
            self.send(Command::VolumeIgnored);
        }
    }

    /// Sets the equalizer, or turns it off with `None`. Changes glide in over about 20 ms and
    /// take effect on audio already queued. Like the volume, it is remembered but not applied
    /// while a stream is bit-perfect; [`PlaybackStatus::eq_active`] says which.
    ///
    /// Boosting can push loud tracks past full scale, where they clip. Pass
    /// [`Equalizer::with_headroom`] to lower the preamp just enough to prevent that.
    pub fn set_equalizer(&self, equalizer: Option<Equalizer>) {
        self.shared.equalizer.set(equalizer);
    }

    /// Fades the end of each track into the start of the next over `fade`, between 1 and 12
    /// seconds (anything else is brought into that range), or turns crossfading off with `None`.
    ///
    /// Only the natural end of a track is crossfaded. Starting another track with
    /// [`Player::play`], seeking and stopping all cut over with the short fade that avoids a
    /// click, as before. Nor are two tracks with the same [`TrackOptions::album_key`] crossfaded,
    /// nor tracks in a different format from the one playing, nor anything in bit-perfect mode:
    /// those join as they would without it, gaplessly where they can.
    ///
    /// The next track has to be queued before the last `fade` plus about half a second of the
    /// current one has been decoded, or it follows without a crossfade; queue it as soon as the
    /// current track starts. [`PlayerEvent::TrackEnded`] and [`PlayerEvent::TrackStarted`] fire
    /// together at the middle of the overlap, where the two tracks are equally loud, and the
    /// status and position switch to the new track then.
    pub fn set_crossfade(&self, fade: Option<Duration>) {
        let ms = fade.map_or(0, |fade| {
            fade.as_millis().clamp(MIN_CROSSFADE_MS, MAX_CROSSFADE_MS) as u32
        });
        self.shared.crossfade_ms.store(ms, Ordering::Relaxed);
    }

    /// Asks for bit-perfect output: the device exclusive to this app, run at the file's own rate
    /// and sample type, with no volume applied. [`PlaybackStatus::bit_perfect`] says whether the
    /// current track got it; when the device cannot, playback carries on normally and a
    /// [`PlayerEvent::Notice`] says why.
    pub fn set_bit_perfect(&self, enabled: bool) {
        self.send(Command::SetBitPerfect(enabled));
    }

    /// A read-only view of what is being played, for the visualizer: the audio after the
    /// equalizer and before the volume, with the time each block is heard. See [`Tap`].
    pub fn tap(&self) -> Tap {
        Tap::new(Arc::clone(&self.shared.tap))
    }

    /// The engine refreshes this every 10 ms, so the position is at most that stale.
    pub fn status(&self) -> PlaybackStatus {
        let mut status = self
            .shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        // Volume is applied immediately, so report it live rather than as of the last refresh;
        // a bit-perfect stream plays at full scale whatever was asked for.
        status.volume = if status.bit_perfect {
            1.0
        } else {
            self.shared.volume.get()
        };
        status.eq_active = status.state != PlaybackState::Stopped
            && !status.bit_perfect
            && self.shared.equalizer.is_on();
        status
    }

    fn send(&self, command: Command) {
        // A send only fails once the engine thread is gone, and there is nothing left to command.
        let _ = self.commands.send(command);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.send(Command::Shutdown);
        if let Some(engine) = self.engine.take() {
            let _ = engine.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_with_camel_case_fields_and_a_type_tag() {
        let json = serde_json::to_value(PlayerEvent::Position {
            track_id: 7,
            position_ms: 1_500,
            duration_ms: 180_000,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "position", "trackId": 7, "positionMs": 1500, "durationMs": 180000
            })
        );
    }

    #[test]
    fn state_changed_flattens_the_status_next_to_the_tag() {
        let mut status = PlaybackStatus::stopped(0.5);
        status.state = PlaybackState::Paused;
        status.track_id = Some(3);
        let json = serde_json::to_value(PlayerEvent::StateChanged(status)).unwrap();
        assert_eq!(json["type"], "stateChanged");
        assert_eq!(json["state"], "paused");
        assert_eq!(json["trackId"], 3);
        assert_eq!(json["outputSampleRate"], serde_json::Value::Null);
    }

    #[test]
    fn error_event_carries_an_optional_track() {
        let json = serde_json::to_value(PlayerEvent::Error {
            track_id: None,
            message: "x".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "error", "trackId": null, "message": "x" })
        );
    }

    #[test]
    fn the_crossfade_is_held_to_one_to_twelve_seconds_and_none_turns_it_off() {
        let (host, _) = crate::host::simulated::Simulated::new(Default::default());
        let player = Player::with_host(|_| {}, move || Box::new(host)).unwrap();
        let set = |fade| {
            player.set_crossfade(fade);
            player.shared.crossfade_ms.load(Ordering::Relaxed)
        };
        assert_eq!(set(Some(Duration::from_millis(100))), 1_000);
        assert_eq!(set(Some(Duration::from_secs(5))), 5_000);
        assert_eq!(set(Some(Duration::from_secs(60))), 12_000);
        assert_eq!(set(None), 0);
    }

    #[test]
    fn the_live_buffer_is_held_to_a_quarter_second_to_half_a_minute() {
        let (host, _) = crate::host::simulated::Simulated::new(Default::default());
        let player = Player::with_host(|_| {}, move || Box::new(host)).unwrap();
        assert_eq!(player.shared.live_buffer_ms.load(Ordering::Relaxed), 2_000);
        let set = |buffer| {
            player.set_live_buffer(buffer);
            player.shared.live_buffer_ms.load(Ordering::Relaxed)
        };
        assert_eq!(set(Duration::from_millis(10)), 250);
        assert_eq!(set(Duration::from_secs(5)), 5_000);
        assert_eq!(set(Duration::from_secs(600)), 30_000);
    }

    #[test]
    fn live_streams_report_themselves_in_the_status_and_events() {
        let mut status = PlaybackStatus::stopped(0.5);
        status.live = true;
        status.buffering = true;
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(
            (&json["live"], &json["buffering"]),
            (&true.into(), &true.into())
        );

        assert_eq!(
            serde_json::to_value(PlayerEvent::Buffering { id: 4 }).unwrap(),
            serde_json::json!({ "type": "buffering", "id": 4 })
        );
        assert_eq!(
            serde_json::to_value(PlayerEvent::LiveEnded {
                id: 4,
                reason: "The stream ended.".into()
            })
            .unwrap(),
            serde_json::json!({ "type": "liveEnded", "id": 4, "reason": "The stream ended." })
        );
    }
}
