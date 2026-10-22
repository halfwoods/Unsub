use std::time::Duration;

use crate::frame::Frame;
use crate::rhythm::{BeatDetector, TempoEstimator};
use crate::ring::Ring;
use crate::scope::Scope;
use crate::signal::Signal;
use crate::spectrum::Spectrum;

/// The FFT length. A Hann window this long is 46 ms at 44.1 kHz.
const FFT_SIZE: usize = 2048;
/// Frames are produced this often, or more: at least [`MIN_FRAME_RATE`] a second.
const MAX_HOP: u32 = 1024;
const MIN_FRAME_RATE: u32 = 60;
/// Slower rates are not worth analysing; the bands would have no bins to read.
const MIN_SAMPLE_RATE: u32 = 8_000;
/// A frame is kept this long.
const HISTORY_SECONDS: f64 = 1.0;
/// A frame is stamped this long after the centre of its window: 0.4 of the window's length less
/// 14 ms, but never earlier than 3 ms before the centre. A hit is recognised once the window has
/// filled with it enough for the spectrum to rise above its average, which happens a little
/// before the window's centre reaches it, by an amount that grows with the window's length. These
/// numbers are a fit to where beats landed on click and kick tracks at rates from 16 to 192 kHz;
/// they put every beat within about 10 ms of its hit at 32 kHz and up. The bands and rms of the
/// frame are those of the window, so they lead the stamp by this much (4.6 ms at 44.1 kHz).
const STAMP_WINDOW_FRACTION: f64 = 0.4;
const STAMP_LATENCY_SECONDS: f64 = 0.014;
const STAMP_EARLIEST_SECONDS: f64 = -0.003;
/// Audio whose presentation time is off by this much from where the previous block ended is a
/// new stretch (a seek or a dropout), not a continuation.
const RESYNC_SECONDS: f64 = 0.025;
/// `frame_at` has nothing to say about a time this far from every frame.
const STALE_SECONDS: f64 = 0.1;
/// An accent of this much in the 0..1 display scale (say every band rising 14 dB) is onset 1.
const ONSET_GAIN: f32 = 5.0;

#[derive(Clone)]
struct Stored {
    /// Presentation time in seconds.
    time: f64,
    frame: Frame,
}

/// Turns the audio that is about to be heard (or is being heard through a microphone) into the
/// [`Frame`]s the visualizer draws.
///
/// Push audio as it is mixed, with the time each block will be heard; ask for the frame at the
/// time now playing. A frame needs the audio half a window past its moment, so the newest frame
/// trails the newest audio by about 20 ms: push at least that far ahead of playback (any output
/// buffer is) and the frame for the playing moment already exists. A frame is made about every
/// 1024 samples, or oftener if that would be fewer than 60 a second.
///
/// ```
/// use std::time::Duration;
/// use unsub_visualize::Analyzer;
///
/// let mut analyzer = Analyzer::new(44_100, 2);
/// // One second of a 440 Hz tone in both channels, first heard at 10 s on the player's clock.
/// let tone: Vec<f32> = (0..44_100)
///     .flat_map(|n| {
///         let sample = 0.5 * (std::f32::consts::TAU * 440.0 * n as f32 / 44_100.0).sin();
///         [sample, sample]
///     })
///     .collect();
/// analyzer.push(&tone, Duration::from_secs(10));
///
/// let frame = analyzer.frame_at(Duration::from_millis(10_500)).expect("a frame");
/// assert!(frame.bands.iter().any(|&band| band > 0.5));
/// ```
///
/// Nothing allocates after construction, so `push` is safe to call from an audio callback.
pub struct Analyzer {
    sample_rate: u32,
    channels: u16,
    hop: usize,
    stamp_offset: i64,
    /// How many samples past a frame's window centre must have arrived before it is made.
    lookahead: u64,
    seconds_per_frame: f64,
    signal: Signal,
    spectrum: Spectrum,
    scope: Scope,
    beats: BeatDetector,
    tempo: TempoEstimator,
    tempo_bpm: Option<f32>,
    frames: Ring<Stored>,
    /// The centre of the window of the next frame to make.
    next_centre: u64,
}

impl Analyzer {
    /// For interleaved `f32` audio at `sample_rate` Hz with `channels` channels. The first two
    /// channels are drawn as left and right; the spectrum is of all of them mixed.
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let sample_rate = sample_rate.max(MIN_SAMPLE_RATE);
        let channels = channels.max(1);
        let hop = (sample_rate / MIN_FRAME_RATE).min(MAX_HOP) as usize;
        let scope = Scope::new(sample_rate);
        let window_seconds = FFT_SIZE as f64 / f64::from(sample_rate);
        let stamp_seconds = (STAMP_WINDOW_FRACTION * window_seconds - STAMP_LATENCY_SECONDS)
            .max(STAMP_EARLIEST_SECONDS);
        let stamp_offset = (stamp_seconds * f64::from(sample_rate)).round() as i64;
        let lookahead = ((FFT_SIZE / 2) as i64)
            .max(stamp_offset + (scope.span() / 2) as i64)
            .unsigned_abs();
        let reach = FFT_SIZE as u64 + scope.span() + stamp_offset.unsigned_abs() + hop as u64;
        let frames_per_second = sample_rate as f32 / hop as f32;
        let kept = (HISTORY_SECONDS * f64::from(frames_per_second)).ceil() as usize + 2;
        Self {
            sample_rate,
            channels,
            hop,
            stamp_offset,
            lookahead,
            seconds_per_frame: hop as f64 / f64::from(sample_rate),
            signal: Signal::new(sample_rate, usize::from(channels), 2 * reach as usize),
            spectrum: Spectrum::new(sample_rate, FFT_SIZE, hop),
            scope,
            beats: BeatDetector::new(frames_per_second),
            tempo: TempoEstimator::new(frames_per_second),
            tempo_bpm: None,
            frames: Ring::new(
                kept,
                Stored {
                    time: 0.0,
                    frame: Frame::silent(),
                },
            ),
            next_centre: 0,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Changes the audio format, forgetting everything if it differs from the current one. This
    /// allocates, so it is for when the format changes (a new song, a different microphone), not
    /// for every block.
    pub fn reconfigure(&mut self, sample_rate: u32, channels: u16) {
        if (sample_rate.max(MIN_SAMPLE_RATE), channels.max(1)) != (self.sample_rate, self.channels)
        {
            *self = Self::new(sample_rate, channels);
        }
    }

    /// Forgets all audio and frames, as when playback stops. The next `push` starts afresh.
    pub fn clear(&mut self) {
        self.signal.close();
        self.start_segment_state();
    }

    /// Takes `interleaved` audio, whose first sample is heard at `at` (a time on any clock, as
    /// long as `frame_at` is asked in the same one). Blocks should follow one another; a block
    /// that starts more than 25 ms from where the last one ended is taken to be a seek or a gap,
    /// and everything before it is forgotten.
    pub fn push(&mut self, interleaved: &[f32], at: Duration) {
        let channels = usize::from(self.channels);
        let whole = interleaved.len() / channels * channels;
        if whole == 0 {
            return;
        }
        let at = at.as_secs_f64();
        if !self.signal.is_open() {
            self.begin_segment(at);
        } else {
            let drift = self.signal.drift(at);
            if drift.abs() > RESYNC_SECONDS {
                self.begin_segment(at);
            } else {
                self.signal.follow(drift);
            }
        }
        // A hop at a time, so the rings never hold more than they have room for.
        for chunk in interleaved[..whole].chunks(self.hop * channels) {
            self.signal.write(chunk);
            while self.next_centre + self.lookahead <= self.signal.total() {
                self.make_frame();
                self.next_centre += self.hop as u64;
            }
        }
    }

    /// The frame nearest `t`, with the continuous values (bands, peaks, rms, onset) blended
    /// between the two frames around `t`. `None` if there is no frame within 100 ms of `t`. The
    /// frame's `t_ms` is that of the nearest real frame, so asking twice for the same beat shows
    /// the same `t_ms`.
    pub fn frame_at(&self, t: Duration) -> Option<Frame> {
        let t = t.as_secs_f64();
        let (age, nearest) = (0..self.frames.len())
            .filter_map(|age| Some((age, self.frames.recent(age)?)))
            .min_by(|(_, a), (_, b)| (a.time - t).abs().total_cmp(&(b.time - t).abs()))?;
        if (nearest.time - t).abs() > STALE_SECONDS {
            return None;
        }
        let mut frame = nearest.frame.clone();
        // Index 0 is the newest frame, so the neighbour after `t` is one younger.
        let neighbour = if t >= nearest.time {
            age.checked_sub(1)
        } else {
            Some(age + 1)
        }
        .and_then(|age| self.frames.recent(age))
        .filter(|other| {
            let gap = other.time - nearest.time;
            gap * (t - nearest.time) > 0.0 && gap.abs() <= 2.0 * self.seconds_per_frame
        });
        if let Some(other) = neighbour {
            let (older, newer) = if other.time > nearest.time {
                (nearest, other)
            } else {
                (other, nearest)
            };
            let weight = ((t - older.time) / (newer.time - older.time)).clamp(0.0, 1.0) as f32;
            blend(
                &mut frame.bands,
                &older.frame.bands,
                &newer.frame.bands,
                weight,
            );
            blend(
                &mut frame.peaks,
                &older.frame.peaks,
                &newer.frame.peaks,
                weight,
            );
            frame.rms = lerp(older.frame.rms, newer.frame.rms, weight);
            frame.onset = lerp(older.frame.onset, newer.frame.onset, weight);
        }
        Some(frame)
    }

    /// The strongest beat marked on a frame made after `after` and no later than `upto` (times on
    /// the clock of `push`), or `None` if there is none.
    ///
    /// A frame is marked as a beat only once the frame after it is made, so a frame that `frame_at`
    /// returned a moment ago may have gained a beat since, and at sample rates that make more than
    /// 60 frames a second some frames are never asked for. Asking for the range since the last
    /// time catches both; see [`Analyzer::beats_known_until`] for how far it can be trusted.
    pub fn beat_between(&self, after: Duration, upto: Duration) -> Option<f32> {
        let (after, upto) = (after.as_secs_f64(), upto.as_secs_f64());
        self.frames
            .iter()
            .filter(|stored| stored.frame.beat && stored.time > after && stored.time <= upto)
            .map(|stored| stored.frame.beat_strength)
            .max_by(f32::total_cmp)
    }

    /// The time of the newest frame whose beat is settled: this and every earlier frame already
    /// has the `beat` it will ever have. The newest frame is not settled, because whether it is a
    /// beat is only learned from the frame after it. `None` until there are two frames.
    pub fn beats_known_until(&self) -> Option<Duration> {
        let settled = self.frames.recent(1)?;
        Some(Duration::try_from_secs_f64(settled.time).unwrap_or_default())
    }

    fn begin_segment(&mut self, at: f64) {
        self.signal.begin_segment(at);
        self.start_segment_state();
    }

    fn start_segment_state(&mut self) {
        self.next_centre = self.signal.segment_start() + (FFT_SIZE / 2) as u64;
        self.frames.clear();
        self.spectrum.reset();
        self.beats.reset();
        self.tempo.reset();
        self.tempo_bpm = None;
    }

    fn make_frame(&mut self) {
        let centre = self.next_centre;
        let stamp = centre.saturating_add_signed(self.stamp_offset);
        let mut frame = Frame::silent();

        let accent =
            self.spectrum
                .analyze(&self.signal, centre, &mut frame.bands, &mut frame.peaks);
        self.scope.capture(
            &self.signal,
            stamp,
            &mut frame.waveform_l,
            &mut frame.waveform_r,
        );
        frame.rms = self
            .signal
            .mean_square(centre - (FFT_SIZE / 2) as u64, FFT_SIZE)
            .sqrt()
            .min(1.0);
        frame.onset = (accent * ONSET_GAIN).min(1.0);

        // A beat is known a frame late, so it is marked on the frame before this one.
        if let Some(strength) = self.beats.push(accent)
            && let Some(previous) = self.frames.recent_mut(0)
        {
            previous.frame.beat = true;
            previous.frame.beat_strength = strength;
        }
        if let Some(bpm) = self.tempo.push(accent) {
            self.tempo_bpm = Some(bpm);
        } else {
            self.tempo_bpm = None;
        }
        frame.tempo_bpm = self.tempo_bpm;

        let time = self.signal.time_of(stamp);
        frame.t_ms = (time * 1000.0).round().max(0.0) as u64;
        self.frames.push(Stored { time, frame });
    }
}

fn lerp(from: f32, to: f32, weight: f32) -> f32 {
    from + (to - from) * weight
}

fn blend<const N: usize>(out: &mut [f32; N], from: &[f32; N], to: &[f32; N], weight: f32) {
    for ((out, &from), &to) in out.iter_mut().zip(from).zip(to) {
        *out = lerp(from, to, weight);
    }
}
