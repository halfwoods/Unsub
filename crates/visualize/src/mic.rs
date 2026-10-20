//! "React to sound": feeds an [`Analyzer`] from a microphone.
//!
//! Audio goes from the device straight into the analyzer's short history and nowhere else: it is
//! never written to a file, a socket or a log, and nothing is kept beyond the analyzer's rings.
//!
//! # Permissions
//!
//! * macOS asks the user for microphone access the first time a stream opens. The app's
//!   `Info.plist` must have `NSMicrophoneUsageDescription` (the sentence the prompt shows), or
//!   macOS ends the process the moment the microphone is touched. An app signed with the hardened
//!   runtime, as a notarized release is, also needs the `com.apple.security.device.audio-input`
//!   entitlement. If the user says no, macOS does not fail the stream: it delivers silence, so a
//!   listener that stays flat may mean access was refused. Some hosts do report it, as
//!   [`Error::PermissionDenied`].
//! * Windows has a similar switch in Settings > Privacy > Microphone, which desktop apps are
//!   subject to on Windows 11; Linux needs the user to be allowed to use the audio device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, InputCallbackInfo, SampleFormat, SizedSample, StreamConfig};
use serde::Serialize;

use crate::{Analyzer, Error, Result};

/// A microphone the user can pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicDevice {
    /// Stable between runs where the system allows; pass it to [`MicListener::start`].
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Listens to a microphone and pushes what it hears into an analyzer, until dropped.
///
/// The analyzer is reconfigured to the microphone's own sample rate and channel count, which it
/// keeps afterwards. Its times are those of [`MicListener::now`].
pub struct MicListener {
    started: Instant,
    failed: Arc<AtomicBool>,
    device_name: String,
    /// Dropping this tells the thread that owns the stream to stop.
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

struct Opened {
    device_name: String,
}

impl MicListener {
    /// The microphones available, the default one marked.
    pub fn devices() -> Result<Vec<MicDevice>> {
        let host = cpal::default_host();
        let default = host
            .default_input_device()
            .and_then(|device| device.id().ok())
            .map(|id| id.to_string());
        let mut devices = Vec::new();
        for device in host.input_devices().map_err(audio_error)? {
            let (Ok(id), Ok(description)) = (device.id(), device.description()) else {
                continue;
            };
            let id = id.to_string();
            devices.push(MicDevice {
                is_default: default.as_deref() == Some(id.as_str()),
                name: description.name().to_owned(),
                id,
            });
        }
        Ok(devices)
    }

    /// Opens `device` (an id or a name from [`MicListener::devices`]; `None` for the system
    /// default) at its native sample rate and starts feeding `analyzer`.
    ///
    /// The first call on macOS shows the permission prompt; see the module documentation.
    pub fn start(device: Option<String>, analyzer: Arc<Mutex<Analyzer>>) -> Result<MicListener> {
        let started = Instant::now();
        let failed = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        // The stream lives on a thread of its own: some audio hosts cannot move a stream between
        // threads, and this keeps the listener `Send` everywhere.
        let thread = {
            let failed = Arc::clone(&failed);
            thread::Builder::new()
                .name("unsub-mic".to_owned())
                .spawn(
                    move || match open(device.as_deref(), analyzer, started, failed) {
                        Ok((stream, opened)) => {
                            let _ = ready_tx.send(Ok(opened));
                            // Returns when the listener is dropped and the sender goes with it.
                            let _ = stop_rx.recv();
                            drop(stream);
                        }
                        Err(error) => {
                            let _ = ready_tx.send(Err(error));
                        }
                    },
                )
                .map_err(|_| Error::ThreadStopped)?
        };
        let opened = ready_rx
            .recv()
            .map_err(|_| Error::ThreadStopped)
            .and_then(|result| result);
        match opened {
            Ok(opened) => Ok(MicListener {
                started,
                failed,
                device_name: opened.device_name,
                stop: Some(stop_tx),
                thread: Some(thread),
            }),
            Err(error) => {
                let _ = thread.join();
                Err(error)
            }
        }
    }

    /// The clock the analyzer's frames are on: ask `frame_at` with this.
    pub fn now(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// False once the device has failed, for example because it was unplugged.
    pub fn is_running(&self) -> bool {
        !self.failed.load(Ordering::Relaxed)
    }
}

impl Drop for MicListener {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn open(
    wanted: Option<&str>,
    analyzer: Arc<Mutex<Analyzer>>,
    started: Instant,
    failed: Arc<AtomicBool>,
) -> Result<(cpal::Stream, Opened)> {
    let host = cpal::default_host();
    let device = match wanted {
        None => host.default_input_device().ok_or(Error::NoInputDevice)?,
        Some(wanted) => host
            .input_devices()
            .map_err(audio_error)?
            .find(|device| {
                device.id().is_ok_and(|id| id.to_string() == wanted)
                    || device
                        .description()
                        .is_ok_and(|description| description.name() == wanted)
            })
            .ok_or_else(|| Error::DeviceNotFound(wanted.to_owned()))?,
    };
    let supported = device.default_input_config().map_err(audio_error)?;
    let config = supported.config();
    {
        let mut analyzer = analyzer.lock().unwrap_or_else(PoisonError::into_inner);
        analyzer.reconfigure(config.sample_rate, config.channels);
        analyzer.clear();
    }

    let on_error = move |error: cpal::Error| {
        if !matches!(
            error.kind(),
            ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged
        ) {
            failed.store(true, Ordering::Relaxed);
        }
    };
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, analyzer, started, on_error),
        SampleFormat::I16 => build::<i16>(&device, config, analyzer, started, on_error),
        SampleFormat::I24 => build::<cpal::I24>(&device, config, analyzer, started, on_error),
        SampleFormat::I32 => build::<i32>(&device, config, analyzer, started, on_error),
        SampleFormat::U16 => build::<u16>(&device, config, analyzer, started, on_error),
        _ => Err(Error::NoSampleFormat),
    }?;
    stream.play().map_err(audio_error)?;
    let device_name = device
        .description()
        .map(|description| description.name().to_owned())
        .unwrap_or_default();
    Ok((stream, Opened { device_name }))
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    analyzer: Arc<Mutex<Analyzer>>,
    started: Instant,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    let rate = f64::from(config.sample_rate.max(1));
    // Room for the largest block seen; it grows once if a device sends more than expected.
    let mut converted: Vec<f32> = Vec::with_capacity(16_384);
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &InputCallbackInfo| {
                converted.clear();
                converted.extend(data.iter().map(|sample| sample.to_sample::<f32>()));
                // The block has just ended; its first sample was heard that long before now.
                let length = Duration::from_secs_f64((data.len() / channels) as f64 / rate);
                let at = started.elapsed().saturating_sub(length);
                analyzer
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(&converted, at);
            },
            on_error,
            None,
        )
        .map_err(audio_error)
}

fn audio_error(error: cpal::Error) -> Error {
    if error.kind() == ErrorKind::PermissionDenied {
        Error::PermissionDenied
    } else {
        Error::Audio(error)
    }
}
