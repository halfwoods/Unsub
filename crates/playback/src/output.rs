use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::gain::{self, Ramp, Volume};
use crate::{Error, Result};

/// Audio the engine can queue ahead of the device. Long enough to ride out a slow disk read,
/// short enough that a seek or track change never waits on stale audio.
const RING_MS: usize = 500;

/// A healthy callback acknowledges a flush within one buffer plus the fade-out.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub sample_rate: u32,
    pub channels: u16,
}

/// State shared between the engine thread and the audio callback; atomics only.
#[derive(Default)]
struct Shared {
    /// Frames the callback has taken from the ring; never reset, so positions can be derived
    /// from differences.
    consumed: AtomicU64,
    paused: AtomicBool,
    /// Bumped by the engine to ask the callback to fade out and discard everything queued.
    flush_request: AtomicU32,
    /// Echoes `flush_request` once the callback has finished that flush.
    flush_ack: AtomicU32,
}

/// Where decoded audio is queued; the output ring in production.
pub trait Sink {
    /// Takes as many whole frames of `samples` as fit and returns how many samples were taken.
    fn push(&mut self, samples: &[f32]) -> usize;
}

/// An open output stream plus the ring buffer that feeds it.
pub struct Output {
    stream: cpal::Stream,
    producer: Producer<f32>,
    shared: Arc<Shared>,
    format: Format,
}

impl Output {
    /// Opens the default device at `source`'s rate and channel count when it supports them,
    /// otherwise at the device's own default. The stream is created paused; call `start`.
    pub fn open(
        source: Format,
        volume: Arc<Volume>,
        on_failure: impl Fn(cpal::Error) + Send + 'static,
    ) -> Result<Output> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(Error::NoDevice)?;
        let (config, sample_format) = choose_config(&device, source)?;
        let format = Format {
            sample_rate: config.sample_rate,
            channels: config.channels,
        };

        let capacity = format.sample_rate as usize * usize::from(format.channels) * RING_MS / 1000;
        let (producer, consumer) = RingBuffer::new(capacity);
        let shared = Arc::new(Shared::default());
        let callback = Callback::new(consumer, Arc::clone(&shared), volume, format);

        let on_error = move |error: cpal::Error| {
            if is_fatal(error.kind()) {
                on_failure(error);
            }
        };
        let stream = match sample_format {
            SampleFormat::F32 => build::<f32>(&device, config, callback, on_error),
            SampleFormat::I32 => build::<i32>(&device, config, callback, on_error),
            SampleFormat::I16 => build::<i16>(&device, config, callback, on_error),
            _ => Err(Error::NoSampleFormat),
        }?;

        Ok(Output {
            stream,
            producer,
            shared,
            format,
        })
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn start(&self) -> Result<()> {
        Ok(self.stream.play()?)
    }

    pub fn consumed(&self) -> u64 {
        self.shared.consumed.load(Ordering::Acquire)
    }

    /// Fades to silence and holds, leaving queued audio in place, or fades back in.
    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    /// Fades out, discards everything queued and returns once the ring is empty. The engine must
    /// not push while this runs.
    pub fn flush(&self) -> Result<()> {
        let request = self
            .shared
            .flush_request
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let deadline = Instant::now() + FLUSH_TIMEOUT;
        while self.shared.flush_ack.load(Ordering::Acquire) != request {
            if Instant::now() >= deadline {
                return Err(Error::OutputStalled);
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}

impl Sink for Output {
    fn push(&mut self, samples: &[f32]) -> usize {
        let channels = usize::from(self.format.channels);
        let count = self.producer.slots().min(samples.len()) / channels * channels;
        // The ring is only ever written whole-frame at a time so the callback can never see a
        // frame split across channels.
        self.producer
            .push_entire_slice(&samples[..count])
            .map_or(0, |()| count)
    }
}

fn is_fatal(kind: ErrorKind) -> bool {
    !matches!(
        kind,
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged
    )
}

fn rank(format: SampleFormat) -> Option<u8> {
    match format {
        SampleFormat::F32 => Some(3),
        SampleFormat::I32 => Some(2),
        SampleFormat::I16 => Some(1),
        _ => None,
    }
}

fn choose_config(device: &cpal::Device, source: Format) -> Result<(StreamConfig, SampleFormat)> {
    let native = device
        .supported_output_configs()?
        .filter(|range| range.channels() == source.channels)
        .filter(|range| rank(range.sample_format()).is_some())
        .filter_map(|range| range.try_with_sample_rate(source.sample_rate))
        .max_by_key(|config| rank(config.sample_format()));

    let config = match native {
        Some(config) => config,
        None => device.default_output_config()?,
    };
    if rank(config.sample_format()).is_none() {
        return Err(Error::NoSampleFormat);
    }
    Ok((config.config(), config.sample_format()))
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

/// The real-time half: runs on the device's thread, so it takes no locks, allocates nothing and
/// does no I/O.
struct Callback {
    consumer: Consumer<f32>,
    shared: Arc<Shared>,
    volume: Arc<Volume>,
    /// Pause, resume and flush fade; the ring is only drained while this is above zero.
    fade: Ramp,
    /// Volume changes. Separate from `fade` so that volume 0 still advances playback.
    level: Ramp,
    channels: usize,
    flushed: u32,
}

impl Callback {
    fn new(
        consumer: Consumer<f32>,
        shared: Arc<Shared>,
        volume: Arc<Volume>,
        format: Format,
    ) -> Self {
        Callback {
            consumer,
            shared,
            fade: Ramp::new(format.sample_rate, 0.0),
            level: Ramp::new(format.sample_rate, gain::curve(volume.get())),
            volume,
            channels: usize::from(format.channels),
            flushed: 0,
        }
    }

    fn render<T: Sample + FromSample<f32>>(&mut self, data: &mut [T]) {
        let request = self.shared.flush_request.load(Ordering::Acquire);
        let flushing = request != self.flushed;
        let hold = flushing || self.shared.paused.load(Ordering::Relaxed);
        let fade_target = if hold { 0.0 } else { 1.0 };
        let level_target = gain::curve(self.volume.get());

        let ready = self.consumer.slots() / self.channels * self.channels;
        let mut taken = 0;
        match self.consumer.read_chunk(ready) {
            Ok(chunk) => {
                let (head, tail) = chunk.as_slices();
                let mut queued = head.iter().chain(tail).copied();
                for frame in data.chunks_exact_mut(self.channels) {
                    let fade = self.fade.next(fade_target);
                    let scale = fade * self.level.next(level_target);
                    if hold && fade == 0.0 {
                        frame.fill(T::EQUILIBRIUM);
                        continue;
                    }
                    for out in frame {
                        *out = match queued.next() {
                            Some(sample) => {
                                taken += 1;
                                T::from_sample(sample * scale)
                            }
                            None => T::EQUILIBRIUM,
                        };
                    }
                }
                chunk.commit(taken);
            }
            Err(_) => data.fill(T::EQUILIBRIUM),
        }
        self.shared
            .consumed
            .fetch_add((taken / self.channels) as u64, Ordering::Release);

        if flushing && self.fade.gain() == 0.0 {
            let queued = self.consumer.slots();
            if let Ok(chunk) = self.consumer.read_chunk(queued) {
                chunk.commit_all();
            }
            self.flushed = request;
            self.shared.flush_ack.store(request, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    struct Rig {
        producer: Producer<f32>,
        shared: Arc<Shared>,
        callback: Callback,
    }

    fn rig(channels: u16, volume: f32) -> Rig {
        let (producer, consumer) = RingBuffer::new(RATE as usize * usize::from(channels));
        let shared = Arc::new(Shared::default());
        let format = Format {
            sample_rate: RATE,
            channels,
        };
        let callback = Callback::new(
            consumer,
            Arc::clone(&shared),
            Arc::new(Volume::new(volume)),
            format,
        );
        Rig {
            producer,
            shared,
            callback,
        }
    }

    fn render(rig: &mut Rig, frames: usize) -> Vec<f32> {
        let mut data = vec![f32::NAN; frames * rig.callback.channels];
        rig.callback.render(&mut data);
        data
    }

    fn consumed(rig: &Rig) -> u64 {
        rig.shared.consumed.load(Ordering::Acquire)
    }

    #[test]
    fn steady_state_output_is_bit_exact() {
        let mut rig = rig(2, 1.0);
        rig.producer.push_entire_slice(&[0.5; 4_000]).unwrap();

        let out = render(&mut rig, 2_000);

        assert!(out[0] < 0.5, "starts faded in");
        assert!(out[1_000..].iter().all(|&sample| sample == 0.5));
        assert_eq!(consumed(&rig), 2_000);
    }

    #[test]
    fn volume_scales_with_the_perceptual_curve() {
        let mut rig = rig(1, 0.5);
        rig.producer.push_entire_slice(&[1.0; 2_000]).unwrap();

        let out = render(&mut rig, 2_000);

        assert_eq!(out[1_999], 0.125);
    }

    #[test]
    fn volume_zero_is_silent_but_playback_still_advances() {
        let mut rig = rig(2, 0.0);
        rig.producer.push_entire_slice(&[0.9; 4_000]).unwrap();

        let out = render(&mut rig, 2_000);

        assert!(out.iter().all(|&sample| sample == 0.0));
        assert_eq!(consumed(&rig), 2_000);
    }

    #[test]
    fn underrun_plays_silence_and_consumes_nothing() {
        let mut rig = rig(2, 1.0);

        let out = render(&mut rig, 256);

        assert!(out.iter().all(|&sample| sample == 0.0));
        assert_eq!(consumed(&rig), 0);
    }

    #[test]
    fn pause_fades_out_then_holds_the_queue_untouched() {
        let mut rig = rig(2, 1.0);
        rig.producer.push_entire_slice(&[0.5; 20_000]).unwrap();
        render(&mut rig, 2_000);

        rig.shared.paused.store(true, Ordering::Relaxed);
        let fading = render(&mut rig, 600);
        assert!(fading.windows(4).all(|w| w[2] <= w[0]), "fade is monotonic");
        assert_eq!(fading[fading.len() - 1], 0.0);
        let held_at = consumed(&rig);

        let silent = render(&mut rig, 4_800);
        assert!(silent.iter().all(|&sample| sample == 0.0));
        assert_eq!(consumed(&rig), held_at, "nothing is consumed while paused");

        rig.shared.paused.store(false, Ordering::Relaxed);
        let resumed = render(&mut rig, 1_000);
        assert_eq!(resumed[1_998], 0.5);
        let queued_frames = rig.callback.consumer.slots() as u64 / 2;
        assert_eq!(
            consumed(&rig) + queued_frames,
            10_000,
            "no frame lost or repeated"
        );
    }

    #[test]
    fn flush_fades_out_discards_the_queue_and_acknowledges() {
        let mut rig = rig(2, 1.0);
        rig.producer.push_entire_slice(&[0.5; 40_000]).unwrap();
        render(&mut rig, 2_000);

        rig.shared.flush_request.fetch_add(1, Ordering::AcqRel);
        let out = render(&mut rig, 960);

        assert_eq!(rig.shared.flush_ack.load(Ordering::Acquire), 1);
        assert!(rig.callback.consumer.is_empty());
        assert!(out[0] > 0.49, "fade starts from the audible level");
        assert!(
            out[..960].windows(4).all(|w| w[2] <= w[0]),
            "fade is monotonic"
        );
        assert_eq!(out[out.len() - 1], 0.0);

        rig.producer.push_entire_slice(&[0.25; 4_000]).unwrap();
        let after = render(&mut rig, 2_000);
        assert!(after[0] < 0.25, "fades back in");
        assert_eq!(after[3_998], 0.25);
    }

    #[test]
    fn flush_while_already_silent_completes_immediately() {
        let mut rig = rig(2, 1.0);
        rig.producer.push_entire_slice(&[0.5; 400]).unwrap();
        rig.shared.flush_request.fetch_add(1, Ordering::AcqRel);

        render(&mut rig, 64);

        assert_eq!(rig.shared.flush_ack.load(Ordering::Acquire), 1);
        assert!(rig.callback.consumer.is_empty());
    }

    #[test]
    fn a_stray_partial_frame_is_left_queued() {
        let mut rig = rig(2, 1.0);
        rig.producer.push_entire_slice(&[0.5; 3]).unwrap();

        let out = render(&mut rig, 8);

        assert_eq!(consumed(&rig), 1);
        assert!(out[2..].iter().all(|&sample| sample == 0.0));
    }

    #[test]
    fn fatal_errors_exclude_recoverable_glitches() {
        assert!(!is_fatal(ErrorKind::Xrun));
        assert!(!is_fatal(ErrorKind::DeviceChanged));
        assert!(is_fatal(ErrorKind::DeviceNotAvailable));
        assert!(is_fatal(ErrorKind::StreamInvalidated));
    }
}
