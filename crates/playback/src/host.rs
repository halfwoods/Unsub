//! The audio system the engine plays through, behind a trait so the engine can be exercised
//! without a sound device.

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{BufferSize, ErrorKind, FromSample, I24, SampleFormat, SizedSample, StreamConfig};

use crate::config::{Offer, depth_label, format_label, is_writable, pick_exact, pick_native};
use crate::output::{Callback, Controls, Format, Output, OutputInfo, Pipe, Request};
use crate::{Error, Result};

/// Called, from any thread but never the audio callback, when a stream has died.
pub type Failure = Box<dyn Fn(Error) + Send>;

pub trait Host {
    /// Opens an output stream for `request`, paused: call `Output::start` once it is primed.
    ///
    /// For an exact request this fails with [`Error::BitPerfectUnavailable`] when the device
    /// cannot carry the source unchanged.
    fn open(
        &mut self,
        request: &Request,
        controls: Controls,
        on_failure: Failure,
    ) -> Result<Output>;

    /// Records the output device's state before playback changes it. `None` if the platform
    /// has nothing to record or restore.
    fn capture(&mut self) -> Option<Box<dyn DeviceState>>;
}

/// The output device's state as found, and its exclusive access.
pub trait DeviceState {
    /// Takes the device for this app alone. `false` if another app holds it or the platform
    /// cannot do it, in which case bit-perfect playback is not possible.
    fn take_exclusive(&mut self) -> bool;

    fn release_exclusive(&mut self);

    /// Puts the device back as captured and gives up exclusive access.
    fn restore(self: Box<Self>);
}

/// The real audio system: CoreAudio, WASAPI or ALSA through cpal, and WASAPI exclusive mode on
/// Windows for bit-perfect output.
pub struct SystemHost {
    /// The output device being played through, remembered from before it was changed.
    ///
    /// A device held in exclusive mode is no longer reported as the system default, so streams
    /// that need it are opened by this identity rather than by asking for the default again.
    #[cfg(target_os = "macos")]
    target: Option<Target>,
    /// Whether this run of playback has asked for exclusive access, after which the default
    /// device cannot be looked up reliably until the run is over.
    #[cfg(target_os = "macos")]
    exclusive_seen: bool,
}

#[cfg(target_os = "macos")]
struct Target {
    id: cpal::DeviceId,
    audio_device: objc2_core_audio::AudioDeviceID,
}

impl SystemHost {
    pub fn new() -> SystemHost {
        SystemHost {
            #[cfg(target_os = "macos")]
            target: None,
            #[cfg(target_os = "macos")]
            exclusive_seen: false,
        }
    }

    /// The device to open a stream on. The system default is followed wherever it moves, until
    /// exclusive access has been asked for; from then on the remembered device is used.
    fn output_device(&self) -> Result<cpal::Device> {
        let host = cpal::default_host();
        #[cfg(target_os = "macos")]
        if self.exclusive_seen
            && let Some(target) = &self.target
        {
            // Enumerated devices open as a plain HAL output unit, the only kind that can run on a
            // device held in exclusive mode; the default device's unit cannot. And just after
            // exclusive access ends the system still reports no default device for a moment.
            return host
                .output_devices()?
                .find(|device| device.id().ok().as_ref() == Some(&target.id))
                .ok_or(Error::NoDevice);
        }
        host.default_output_device().ok_or(Error::NoDevice)
    }
}

impl Host for SystemHost {
    fn open(
        &mut self,
        request: &Request,
        controls: Controls,
        on_failure: Failure,
    ) -> Result<Output> {
        #[cfg(windows)]
        if request.exact {
            return crate::wasapi_exclusive::open(request, controls, on_failure);
        }
        #[cfg(target_os = "macos")]
        if request.exact {
            self.exclusive_seen = true;
        }
        let device = self.output_device()?;
        self.open_cpal(&device, request, controls, on_failure)
    }

    fn capture(&mut self) -> Option<Box<dyn DeviceState>> {
        #[cfg(target_os = "macos")]
        {
            let saved = crate::device_state::Saved::capture()?;
            let id = cpal::default_host().default_output_device()?.id().ok()?;
            self.target = Some(Target {
                id,
                audio_device: saved.device(),
            });
            self.exclusive_seen = false;
            Some(Box::new(saved))
        }
        #[cfg(windows)]
        {
            // Exclusivity belongs to the exclusive-mode stream itself, and shared-mode playback
            // changes nothing on the system that would need undoing.
            Some(Box::new(PerStream))
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            None
        }
    }
}

/// Platforms where holding the device is part of opening the stream.
#[cfg(windows)]
struct PerStream;

#[cfg(windows)]
impl DeviceState for PerStream {
    fn take_exclusive(&mut self) -> bool {
        true
    }

    fn release_exclusive(&mut self) {}

    fn restore(self: Box<Self>) {}
}

impl SystemHost {
    fn open_cpal(
        &self,
        device: &cpal::Device,
        request: &Request,
        controls: Controls,
        on_failure: Failure,
    ) -> Result<Output> {
        let offers = offers(device)?;
        let source = request.source;

        let (config, sample_format) = if request.exact {
            let format = pick_exact(&offers, source, request.bit_depth)
                .ok_or_else(|| Error::BitPerfectUnavailable(unmatched(source)))?;
            (stream_config(source), format)
        } else if let Some(format) = pick_native(&offers, source) {
            (stream_config(source), format)
        } else {
            let default = device.default_output_config()?;
            if !is_writable(default.sample_format()) {
                return Err(Error::NoSampleFormat);
            }
            (default.config(), default.sample_format())
        };
        let format = Format {
            sample_rate: config.sample_rate,
            channels: config.channels,
        };

        let (callback, pipe) = Pipe::new(format, controls, request.exact);
        let on_error = move |error: cpal::Error| {
            if is_fatal(error.kind()) {
                on_failure(Error::Output(error));
            }
        };
        let stream: Box<dyn crate::output::Stream> = match sample_format {
            SampleFormat::F32 => Box::new(build::<f32>(device, config, callback, on_error)?),
            SampleFormat::I32 => Box::new(build::<i32>(device, config, callback, on_error)?),
            SampleFormat::I24 => Box::new(build::<I24>(device, config, callback, on_error)?),
            SampleFormat::I16 => Box::new(build::<i16>(device, config, callback, on_error)?),
            _ => return Err(Error::NoSampleFormat),
        };

        let info = self.describe(request, sample_format, format)?;
        Ok(pipe.into_output(stream, format, *request, info))
    }

    /// What the stream just opened is, checked against the hardware where the platform lets us.
    fn describe(
        &self,
        request: &Request,
        sample_format: SampleFormat,
        format: Format,
    ) -> Result<OutputInfo> {
        let label = format_label(depth_label(sample_format), format.sample_rate);
        #[cfg(target_os = "macos")]
        if let Some(physical) = self
            .target
            .as_ref()
            .and_then(|target| crate::device_state::physical_format(target.audio_device))
        {
            let depth = if physical.float {
                "32-bit float".to_owned()
            } else {
                format!("{}-bit", physical.bits)
            };
            let label = format_label(&depth, physical.sample_rate);
            if request.exact && !physical_is_exact(physical, request) {
                return Err(Error::BitPerfectUnavailable(format!(
                    "the device is running at {label}"
                )));
            }
            return Ok(OutputInfo {
                bit_perfect: request.exact,
                label,
            });
        }
        Ok(OutputInfo {
            bit_perfect: request.exact,
            label,
        })
    }
}

fn stream_config(source: Format) -> StreamConfig {
    StreamConfig {
        channels: source.channels,
        sample_rate: source.sample_rate,
        buffer_size: BufferSize::Default,
    }
}

fn offers(device: &cpal::Device) -> Result<Vec<Offer>> {
    Ok(device
        .supported_output_configs()?
        .map(|range| Offer {
            channels: range.channels(),
            min_rate: range.min_sample_rate(),
            max_rate: range.max_sample_rate(),
            format: range.sample_format(),
        })
        .collect())
}

fn unmatched(source: Format) -> String {
    format!(
        "the device has no {}-channel {} format",
        source.channels,
        format_label("", source.sample_rate).trim()
    )
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut callback: Callback,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| callback.render(data),
        on_error,
        None,
    )?)
}

/// Whether hardware running as `physical` carries the source untouched: same rate, and a sample
/// type with room for every bit (float holds 24).
#[cfg(target_os = "macos")]
fn physical_is_exact(physical: crate::device_state::PhysicalFormat, request: &Request) -> bool {
    let room = if physical.float { 24 } else { physical.bits };
    physical.sample_rate == request.source.sample_rate && room >= request.bit_depth.unwrap_or(0)
}

fn is_fatal(kind: ErrorKind) -> bool {
    !matches!(
        kind,
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged
    )
}

/// A stand-in audio system for tests: a thread that drains the ring as a device would, at many
/// times real speed, and records what it was asked to do.
#[cfg(test)]
pub(crate) mod simulated {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use super::*;
    use crate::config::{depth_label, format_label};
    use crate::output::Stream;

    pub struct Config {
        /// Rates the device can run at exactly.
        pub rates: Vec<u32>,
        /// Whether another app already holds the device.
        pub held_elsewhere: bool,
        /// The rate (stereo) the device falls back to for anything else.
        pub default_rate: u32,
        /// How many times faster than real time the device consumes audio.
        pub speed: u32,
    }

    impl Default for Config {
        fn default() -> Self {
            Config {
                rates: vec![44_100, 48_000, 88_200, 96_000],
                held_elsewhere: false,
                default_rate: 48_000,
                speed: 20,
            }
        }
    }

    #[derive(Default)]
    pub struct Log {
        /// Every output opened, in order.
        pub opened: Vec<Request>,
        /// Device-state calls in order: "capture", "take_exclusive", "release_exclusive",
        /// "restore".
        pub device: Vec<&'static str>,
        /// Everything the device played, silence included.
        pub heard: Vec<f32>,
        /// Whether this app holds the device right now.
        pub exclusive: bool,
        /// Whether a stream is open right now.
        pub streaming: bool,
    }

    pub struct Simulated {
        config: Config,
        log: Arc<Mutex<Log>>,
    }

    impl Simulated {
        pub fn new(config: Config) -> (Simulated, Arc<Mutex<Log>>) {
            let log = Arc::new(Mutex::new(Log::default()));
            (
                Simulated {
                    config,
                    log: Arc::clone(&log),
                },
                log,
            )
        }
    }

    fn locked(log: &Mutex<Log>) -> std::sync::MutexGuard<'_, Log> {
        log.lock().unwrap_or_else(PoisonError::into_inner)
    }

    impl Host for Simulated {
        fn open(&mut self, request: &Request, controls: Controls, _: Failure) -> Result<Output> {
            let source = request.source;
            let exact_rate = self.config.rates.contains(&source.sample_rate);
            if request.exact && !exact_rate {
                return Err(Error::BitPerfectUnavailable(unmatched(source)));
            }
            let format = if exact_rate {
                source
            } else {
                Format {
                    sample_rate: self.config.default_rate,
                    channels: 2,
                }
            };
            locked(&self.log).opened.push(*request);

            let (callback, pipe) = Pipe::new(format, controls, request.exact);
            let stream = Box::new(SimulatedStream {
                callback: Mutex::new(Some(callback)),
                format,
                speed: self.config.speed,
                log: Arc::clone(&self.log),
                stop: Arc::new(AtomicBool::new(false)),
                thread: Mutex::new(None),
            });
            let info = OutputInfo {
                bit_perfect: request.exact,
                label: format_label(depth_label(SampleFormat::F32), format.sample_rate),
            };
            Ok(pipe.into_output(stream, format, *request, info))
        }

        fn capture(&mut self) -> Option<Box<dyn DeviceState>> {
            locked(&self.log).device.push("capture");
            Some(Box::new(SimulatedDevice {
                held_elsewhere: self.config.held_elsewhere,
                log: Arc::clone(&self.log),
            }))
        }
    }

    struct SimulatedDevice {
        held_elsewhere: bool,
        log: Arc<Mutex<Log>>,
    }

    impl DeviceState for SimulatedDevice {
        fn take_exclusive(&mut self) -> bool {
            let mut log = locked(&self.log);
            log.device.push("take_exclusive");
            log.exclusive = !self.held_elsewhere;
            log.exclusive
        }

        fn release_exclusive(&mut self) {
            let mut log = locked(&self.log);
            if log.exclusive {
                log.device.push("release_exclusive");
                log.exclusive = false;
            }
        }

        fn restore(self: Box<Self>) {
            let mut log = locked(&self.log);
            log.device.push("restore");
            log.exclusive = false;
        }
    }

    struct SimulatedStream {
        callback: Mutex<Option<Callback>>,
        format: Format,
        speed: u32,
        log: Arc<Mutex<Log>>,
        stop: Arc<AtomicBool>,
        thread: Mutex<Option<JoinHandle<()>>>,
    }

    impl Stream for SimulatedStream {
        fn start(&self) -> Result<()> {
            let Some(mut callback) = self
                .callback
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            else {
                return Ok(());
            };
            let (format, speed) = (self.format, self.speed);
            let (log, stop) = (Arc::clone(&self.log), Arc::clone(&self.stop));
            locked(&log).streaming = true;
            let thread = thread::spawn(move || {
                const PERIOD_FRAMES: usize = 256;
                let pause = Duration::from_secs_f64(
                    PERIOD_FRAMES as f64 / f64::from(format.sample_rate) / f64::from(speed),
                );
                let mut buffer = vec![0.0_f32; PERIOD_FRAMES * usize::from(format.channels)];
                while !stop.load(Ordering::Relaxed) {
                    callback.render(&mut buffer);
                    locked(&log).heard.extend_from_slice(&buffer);
                    thread::sleep(pause);
                }
            });
            *self.thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread);
            Ok(())
        }
    }

    impl Drop for SimulatedStream {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let thread = self
                .thread
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(thread) = thread {
                let _ = thread.join();
            }
            locked(&self.log).streaming = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fatal_errors_exclude_recoverable_glitches() {
        assert!(!is_fatal(ErrorKind::Xrun));
        assert!(!is_fatal(ErrorKind::DeviceChanged));
        assert!(is_fatal(ErrorKind::DeviceNotAvailable));
        assert!(is_fatal(ErrorKind::StreamInvalidated));
    }

    #[test]
    fn the_unmatched_reason_names_what_the_device_lacks() {
        let reason = unmatched(Format {
            sample_rate: 352_800,
            channels: 2,
        });
        assert_eq!(reason, "the device has no 2-channel 352.8 kHz format");
    }
}
