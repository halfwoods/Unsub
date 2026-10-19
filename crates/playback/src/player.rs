use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use serde::Serialize;

use crate::Result;
use crate::engine::{Command, Engine};
use crate::gain::Volume;
use crate::host::{Host, SystemHost};

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
}

/// State the engine thread publishes and the [`Player`] reads.
pub(crate) struct Shared {
    /// Applied by the audio callback; ignored while a stream is bit-perfect.
    pub volume: Arc<Volume>,
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
            bit_perfect: AtomicBool::new(false),
        });

        let engine = {
            let (sender, shared) = (commands.clone(), Arc::clone(&shared));
            // The engine owns the audio stream, which is not `Send` on every platform, so it is
            // built on the thread that will use it.
            thread::Builder::new()
                .name("antistreaming-playback".into())
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
        self.send(Command::Play(track_id, path));
    }

    /// Plays `path` straight after the current track, without a gap when the format matches.
    /// Replaces any previously queued track.
    pub fn queue_next(&self, track_id: i64, path: PathBuf) {
        self.send(Command::QueueNext(track_id, path));
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

    /// Asks for bit-perfect output: the device exclusive to this app, run at the file's own rate
    /// and sample type, with no volume applied. [`PlaybackStatus::bit_perfect`] says whether the
    /// current track got it; when the device cannot, playback carries on normally and a
    /// [`PlayerEvent::Notice`] says why.
    pub fn set_bit_perfect(&self, enabled: bool) {
        self.send(Command::SetBitPerfect(enabled));
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
}
