use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use cpal::{FromSample, SampleFormat, SizedSample};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::equalizer::{Chain, EqControl};
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

/// What the user can change while a stream plays, read by the callback without locking.
#[derive(Clone)]
pub struct Controls {
    pub volume: Arc<Volume>,
    pub equalizer: Arc<EqControl>,
}

/// What the engine asks of an output stream for one source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub source: Format,
    /// Bits per sample in the file, used to pick an integer device format in exact mode.
    pub bit_depth: Option<u32>,
    /// Bit-perfect: the device runs at exactly the source's rate and channels, volume is fixed at
    /// unity, and the device is exclusive.
    pub exact: bool,
}

/// What an open stream turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputInfo {
    /// Whether the stream really is bit-perfect, which is only so if exact mode was requested
    /// and the device honoured all of it.
    pub bit_perfect: bool,
    /// The format the device is fed, e.g. "24-bit 96 kHz".
    pub label: String,
}

/// State shared between the engine thread and the audio callback; atomics only.
#[derive(Default)]
pub struct Shared {
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

/// A device stream whose callback is already wired to a ring. Dropping it closes the stream.
pub trait Stream {
    fn start(&self) -> Result<()>;
}

impl Stream for cpal::Stream {
    fn start(&self) -> Result<()> {
        use cpal::traits::StreamTrait;
        Ok(self.play()?)
    }
}

/// An open output stream plus the ring buffer that feeds it.
pub struct Output {
    stream: Box<dyn Stream>,
    producer: Producer<f32>,
    shared: Arc<Shared>,
    format: Format,
    request: Request,
    info: OutputInfo,
}

/// The engine's end of a new output's plumbing: the ring's write end and the shared flags.
pub struct Pipe {
    producer: Producer<f32>,
    shared: Arc<Shared>,
}

impl Pipe {
    /// The callback goes to the backend's device; the pipe stays to become the [`Output`].
    pub fn new(format: Format, controls: Controls, exact: bool) -> (Callback, Pipe) {
        let capacity = format.sample_rate as usize * usize::from(format.channels) * RING_MS / 1000;
        let (producer, consumer) = RingBuffer::new(capacity);
        let shared = Arc::new(Shared::default());
        let callback = Callback::new(consumer, Arc::clone(&shared), controls, format, exact);
        (callback, Pipe { producer, shared })
    }

    /// Finishes the output once the backend has built `stream` around the callback.
    pub fn into_output(
        self,
        stream: Box<dyn Stream>,
        format: Format,
        request: Request,
        info: OutputInfo,
    ) -> Output {
        Output {
            stream,
            producer: self.producer,
            shared: self.shared,
            format,
            request,
            info,
        }
    }
}

impl Output {
    /// The device's format, which differs from the source's when the device cannot match it.
    pub fn format(&self) -> Format {
        self.format
    }

    pub fn request(&self) -> &Request {
        &self.request
    }

    pub fn info(&self) -> &OutputInfo {
        &self.info
    }

    pub fn start(&self) -> Result<()> {
        self.stream.start()
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

/// The real-time half: runs on the device's thread, so it takes no locks, allocates nothing and
/// does no I/O.
pub struct Callback {
    consumer: Consumer<f32>,
    shared: Arc<Shared>,
    volume: Arc<Volume>,
    /// Bit-perfect streams ignore the volume and the equalizer and stay at unity.
    exact: bool,
    /// Applied to the decoded samples before the volume; absent when bit-perfect.
    equalizer: Option<Chain>,
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
        controls: Controls,
        format: Format,
        exact: bool,
    ) -> Self {
        let channels = usize::from(format.channels);
        Callback {
            consumer,
            shared,
            fade: Ramp::new(format.sample_rate, 0.0),
            level: Ramp::new(format.sample_rate, level_target(&controls.volume, exact)),
            volume: controls.volume,
            exact,
            equalizer: (!exact)
                .then(|| Chain::new(controls.equalizer, format.sample_rate, channels)),
            channels,
            flushed: 0,
        }
    }

    /// How many of `period` frames a simulated device should render now. Pause and flush render
    /// silence by design, so they get the whole period; otherwise only the frames the engine has
    /// supplied, so that a device running far faster than real time does not turn the engine's
    /// scheduling delays into dropouts a real-time device would never have had.
    #[cfg(test)]
    pub fn frames_to_render(&self, period: usize) -> usize {
        let held = self.shared.flush_request.load(Ordering::Acquire) != self.flushed
            || self.shared.paused.load(Ordering::Relaxed);
        if held {
            period
        } else {
            (self.consumer.slots() / self.channels).min(period)
        }
    }

    pub fn render<T: SizedSample + FromSample<f32>>(&mut self, data: &mut [T]) {
        let request = self.shared.flush_request.load(Ordering::Acquire);
        let flushing = request != self.flushed;
        let hold = flushing || self.shared.paused.load(Ordering::Relaxed);
        let fade_target = if hold { 0.0 } else { 1.0 };
        let level_target = level_target(&self.volume, self.exact);
        if let Some(equalizer) = self.equalizer.as_mut() {
            equalizer.sync();
        }

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
                    if let Some(equalizer) = self.equalizer.as_mut() {
                        equalizer.advance();
                    }
                    for (channel, out) in frame.iter_mut().enumerate() {
                        *out = match queued.next() {
                            Some(sample) => {
                                taken += 1;
                                let sample = match self.equalizer.as_mut() {
                                    Some(equalizer) => equalizer.process(channel, sample),
                                    None => sample,
                                };
                                to_device(sample * scale)
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
            if let Some(equalizer) = self.equalizer.as_mut() {
                equalizer.reset();
            }
            self.flushed = request;
            self.shared.flush_ack.store(request, Ordering::Release);
        }
    }
}

fn level_target(volume: &Volume, exact: bool) -> f32 {
    if exact {
        1.0
    } else {
        gain::curve(volume.get())
    }
}

/// Converts to the device's sample type. Integer formats cannot represent +1.0 or anything
/// beyond, and cpal's 24-bit type does not saturate, so out-of-range floats are clipped first;
/// float formats pass through untouched.
fn to_device<T: SizedSample + FromSample<f32>>(sample: f32) -> T {
    if matches!(T::FORMAT, SampleFormat::F32 | SampleFormat::F64) {
        T::from_sample(sample)
    } else {
        T::from_sample(sample.clamp(-1.0, 1.0 - f32::EPSILON / 2.0))
    }
}

#[cfg(test)]
mod tests {
    use cpal::I24;

    use super::*;
    use crate::equalizer::Equalizer;

    const RATE: u32 = 48_000;

    struct Rig {
        producer: Producer<f32>,
        shared: Arc<Shared>,
        callback: Callback,
        equalizer: Arc<EqControl>,
    }

    fn rig(channels: u16, volume: f32) -> Rig {
        rig_with(channels, volume, false)
    }

    fn rig_with(channels: u16, volume: f32, exact: bool) -> Rig {
        let (producer, consumer) = RingBuffer::new(RATE as usize * usize::from(channels));
        let shared = Arc::new(Shared::default());
        let format = Format {
            sample_rate: RATE,
            channels,
        };
        let equalizer = Arc::new(EqControl::default());
        let controls = Controls {
            volume: Arc::new(Volume::new(volume)),
            equalizer: Arc::clone(&equalizer),
        };
        let callback = Callback::new(consumer, Arc::clone(&shared), controls, format, exact);
        Rig {
            producer,
            shared,
            callback,
            equalizer,
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
    fn a_simulated_device_takes_only_what_is_queued_unless_it_is_held() {
        let mut rig = rig(2, 1.0);
        assert_eq!(rig.callback.frames_to_render(256), 0, "nothing queued");

        rig.producer.push_entire_slice(&[0.5; 200]).unwrap();
        assert_eq!(rig.callback.frames_to_render(256), 100, "a partial period");

        rig.producer.push_entire_slice(&[0.5; 1_000]).unwrap();
        assert_eq!(
            rig.callback.frames_to_render(256),
            256,
            "capped at a period"
        );

        rig.callback
            .consumer
            .read_chunk(1_200)
            .unwrap()
            .commit_all();
        rig.shared.paused.store(true, Ordering::Relaxed);
        assert_eq!(
            rig.callback.frames_to_render(256),
            256,
            "silence is played whole"
        );

        rig.shared.paused.store(false, Ordering::Relaxed);
        rig.shared.flush_request.fetch_add(1, Ordering::AcqRel);
        assert_eq!(
            rig.callback.frames_to_render(256),
            256,
            "so is the fade of a flush"
        );
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

    fn render_as<T: SizedSample + FromSample<f32>>(rig: &mut Rig, frames: usize) -> Vec<T> {
        let mut data = vec![T::EQUILIBRIUM; frames * rig.callback.channels];
        rig.callback.render(&mut data);
        data
    }

    /// Plays `values` (as source samples `value / scale`) through a bit-perfect rig after a lead-in
    /// of silence that absorbs the fade-in, and returns what the device received for them.
    fn play_exactly<T: SizedSample + FromSample<f32>>(values: &[i32], scale: f32) -> Vec<T> {
        let lead = 1_000;
        let mut rig = rig_with(1, 0.25, true);
        rig.producer.push_entire_slice(&vec![0.0; lead]).unwrap();
        let source: Vec<f32> = values.iter().map(|&v| v as f32 / scale).collect();
        rig.producer.push_entire_slice(&source).unwrap();
        let mut out = render_as::<T>(&mut rig, lead + values.len());
        out.split_off(lead)
    }

    #[test]
    fn sixteen_bit_samples_reach_a_sixteen_bit_device_unchanged() {
        let values: Vec<i32> = (-32_768..32_768).step_by(2).collect();
        let out = play_exactly::<i16>(&values, 32_768.0);
        assert!(out.iter().map(|&v| i32::from(v)).eq(values.iter().copied()));
    }

    #[test]
    fn twenty_four_bit_samples_reach_a_24_bit_device_unchanged() {
        let mut values: Vec<i32> = (-8_388_608..8_388_608).step_by(997).collect();
        values.extend([-8_388_608, -1, 0, 1, 8_388_607]);
        let out = play_exactly::<I24>(&values, 8_388_608.0);
        assert!(out.iter().map(|v| v.inner()).eq(values.iter().copied()));
    }

    #[test]
    fn twenty_four_bit_samples_reach_a_32_bit_device_unchanged_in_its_top_bits() {
        let values: Vec<i32> = (-8_388_608..8_388_608).step_by(997).collect();
        let out = play_exactly::<i32>(&values, 8_388_608.0);
        assert!(out.iter().copied().eq(values.iter().map(|&v| v << 8)));
    }

    #[test]
    fn a_float_device_carries_sixteen_and_24_bit_samples_unchanged() {
        let values: Vec<i32> = (-8_388_608..8_388_608).step_by(997).collect();
        let out = play_exactly::<f32>(&values, 8_388_608.0);
        assert!(
            out.iter()
                .copied()
                .eq(values.iter().map(|&v| v as f32 / 8_388_608.0))
        );
    }

    #[test]
    fn a_bit_perfect_stream_ignores_the_volume() {
        let mut exact = rig_with(1, 0.25, true);
        exact.producer.push_entire_slice(&[0.5; 2_000]).unwrap();
        assert_eq!(render(&mut exact, 2_000)[1_999], 0.5);

        let mut shared = rig_with(1, 0.25, false);
        shared.producer.push_entire_slice(&[0.5; 2_000]).unwrap();
        assert!(
            render(&mut shared, 2_000)[1_999] < 0.01,
            "cubic curve of 0.25"
        );
    }

    #[test]
    fn the_equalizer_shapes_a_normal_stream_and_leaves_a_bit_perfect_one_alone() {
        let quieter = Equalizer {
            preamp_db: -6.0,
            bands: [0.0; 10],
        };
        for (exact, expected) in [(false, 0.5 * 10f32.powf(-6.0 / 20.0)), (true, 0.5)] {
            let mut rig = rig_with(1, 1.0, exact);
            rig.equalizer.set(Some(quieter));
            rig.producer.push_entire_slice(&[0.5; 4_000]).unwrap();
            let out = render(&mut rig, 4_000);
            // The change glides in over 20 ms and the stream fades in over 10, so look at the end.
            assert!(
                (out[3_999] - expected).abs() < 1e-5,
                "exact {exact}: {}",
                out[3_999]
            );
        }
    }

    #[test]
    fn integer_devices_clip_out_of_range_samples_instead_of_wrapping() {
        let mut rig = rig_with(1, 1.0, true);
        rig.producer.push_entire_slice(&[0.0; 1_000]).unwrap();
        rig.producer.push_entire_slice(&[1.5, -1.5, 1.0]).unwrap();
        let out = render_as::<i16>(&mut rig, 1_003);
        assert_eq!(&out[1_000..], [i16::MAX, i16::MIN, i16::MAX]);

        let mut rig = rig_with(1, 1.0, true);
        rig.producer.push_entire_slice(&[0.0; 1_000]).unwrap();
        rig.producer.push_entire_slice(&[1.5, -1.5]).unwrap();
        let out = render_as::<I24>(&mut rig, 1_002);
        assert_eq!(
            (out[1_000].inner(), out[1_001].inner()),
            (8_388_607, -8_388_608)
        );
    }

    #[test]
    fn float_devices_receive_values_beyond_full_scale_as_they_are() {
        let mut rig = rig_with(1, 1.0, true);
        rig.producer.push_entire_slice(&[0.0; 1_000]).unwrap();
        rig.producer.push_entire_slice(&[1.5, -1.5]).unwrap();
        let out = render_as::<f32>(&mut rig, 1_002);
        assert_eq!(&out[1_000..], [1.5, -1.5]);
    }
}
