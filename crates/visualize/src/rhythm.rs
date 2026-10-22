//! Beat and tempo, both read from the stream of accents the spectrum produces, one per frame.

use crate::ring::Ring;

/// A beat needs an accent this many standard deviations above its recent mean...
const BEAT_SIGMAS: f32 = 2.0;
/// ...and never less than this: about 3.5 dB across every band at once, which is above the
/// flicker of even steady noise (its accents reach 0.045), and below a kick under a bass line
/// or a snare.
const BEAT_MIN_FLUX: f32 = 0.05;
/// How much recent accent the threshold adapts to.
const BEAT_HISTORY_SECONDS: f32 = 1.0;
/// Two beats are never closer than this (a limit of about 400 BPM).
const BEAT_MIN_GAP_SECONDS: f32 = 0.15;

/// Picks beats out of the accents: a local peak that stands well above the average of the last
/// second. Any sudden hit counts, not only a kick: a bass line sits on a kick and hides it from
/// the bass alone, while a hit shows across the whole spectrum. A peak is only known once the
/// next frame has arrived, so a beat is reported one frame after the frame it belongs to.
pub(crate) struct BeatDetector {
    history: Ring<f32>,
    /// The accent of the last two frames, oldest first.
    last: [f32; 2],
    quiet_for: f32,
    seconds_per_frame: f32,
}

impl BeatDetector {
    pub(crate) fn new(frames_per_second: f32) -> Self {
        Self {
            history: Ring::new(
                (BEAT_HISTORY_SECONDS * frames_per_second).ceil() as usize,
                0.0,
            ),
            last: [0.0; 2],
            quiet_for: 0.0,
            seconds_per_frame: 1.0 / frames_per_second,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.history.clear();
        self.last = [0.0; 2];
        self.quiet_for = 0.0;
    }

    /// Takes the newest frame's accent. Returns the strength (0..1) of a beat on the frame
    /// before it, if there was one.
    pub(crate) fn push(&mut self, accent: f32) -> Option<f32> {
        let [before, candidate] = self.last;
        self.last = [candidate, accent];
        self.history.push(candidate);
        self.quiet_for -= self.seconds_per_frame;

        let threshold = self.threshold();
        let is_peak = candidate >= before && candidate > accent;
        (is_peak && candidate > threshold && self.quiet_for <= 0.0).then(|| {
            self.quiet_for = BEAT_MIN_GAP_SECONDS - self.seconds_per_frame;
            (1.0 - threshold / candidate).clamp(0.0, 1.0)
        })
    }

    fn threshold(&self) -> f32 {
        let count = self.history.len() as f32;
        let mean = self.history.iter().sum::<f32>() / count;
        let variance = self
            .history
            .iter()
            .map(|flux| (flux - mean).powi(2))
            .sum::<f32>()
            / count;
        (mean + BEAT_SIGMAS * variance.sqrt()).max(BEAT_MIN_FLUX)
    }
}

/// The slowest and fastest tempi looked for.
const TEMPO_MIN_BPM: f32 = 60.0;
const TEMPO_MAX_BPM: f32 = 200.0;
/// How much rhythm is looked at, and the least that must have been heard before guessing.
const TEMPO_HISTORY_SECONDS: f32 = 8.0;
const TEMPO_MIN_SECONDS: f32 = 4.0;
/// How often the estimate is redone.
const TEMPO_EVERY_SECONDS: f32 = 0.5;
/// The correlation (1 for a perfectly repeating pattern) a rhythm needs to count as one.
const TEMPO_MIN_CORRELATION: f32 = 0.25;
/// A beat period half as long wins over the strongest peak if it is nearly as strong, which
/// settles on the beat rather than every second beat.
const TEMPO_HALF_PERIOD_RATIO: f32 = 0.75;
/// Estimates within this fraction of one another count as the same tempo.
const TEMPO_AGREEMENT: f32 = 0.025;
const TEMPO_AGREE_COUNT: usize = 3;

/// Finds the tempo by autocorrelating the accents over the last few seconds, and reports it only
/// after several estimates in a row agree.
pub(crate) struct TempoEstimator {
    flux: Ring<f32>,
    centred: Vec<f32>,
    correlation: Vec<f32>,
    recent: Ring<f32>,
    frames_per_second: f32,
    every: usize,
    since_estimate: usize,
}

impl TempoEstimator {
    pub(crate) fn new(frames_per_second: f32) -> Self {
        let capacity = (TEMPO_HISTORY_SECONDS * frames_per_second).ceil() as usize;
        Self {
            flux: Ring::new(capacity, 0.0),
            centred: vec![0.0; capacity],
            correlation: vec![0.0; (frames_per_second * 60.0 / TEMPO_MIN_BPM).ceil() as usize + 2],
            recent: Ring::new(TEMPO_AGREE_COUNT, 0.0),
            frames_per_second,
            every: (TEMPO_EVERY_SECONDS * frames_per_second).ceil() as usize,
            since_estimate: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.flux.clear();
        self.recent.clear();
        self.since_estimate = 0;
    }

    /// Takes the newest frame's accent; returns the tempo if it has settled.
    pub(crate) fn push(&mut self, accent: f32) -> Option<f32> {
        self.flux.push(accent);
        self.since_estimate += 1;
        if self.since_estimate >= self.every {
            self.since_estimate = 0;
            match self.estimate() {
                Some(bpm) => self.recent.push(bpm),
                None => self.recent.clear(),
            }
        }
        self.settled()
    }

    /// The median of the recent estimates, if enough of them agree.
    fn settled(&self) -> Option<f32> {
        if self.recent.len() < TEMPO_AGREE_COUNT {
            return None;
        }
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        let mut sum = 0.0;
        for &bpm in self.recent.iter() {
            low = low.min(bpm);
            high = high.max(bpm);
            sum += bpm;
        }
        let mean = sum / TEMPO_AGREE_COUNT as f32;
        ((high - low) <= TEMPO_AGREEMENT * mean).then_some(mean)
    }

    fn estimate(&mut self) -> Option<f32> {
        let count = self.flux.len();
        if (count as f32) < TEMPO_MIN_SECONDS * self.frames_per_second {
            return None;
        }
        // A sound with no jump in it big enough to be a beat has no tempo, however regular the
        // ripples in it are.
        if self.flux.iter().all(|&flux| flux < BEAT_MIN_FLUX) {
            return None;
        }
        let mean = self.flux.iter().sum::<f32>() / count as f32;
        for (slot, &flux) in self.centred.iter_mut().zip(self.flux.iter()) {
            *slot = flux - mean;
        }
        let series = &self.centred[..count];
        let variance = series.iter().map(|value| value * value).sum::<f32>() / count as f32;
        if variance < 1e-8 {
            return None;
        }

        let shortest = (self.frames_per_second * 60.0 / TEMPO_MAX_BPM)
            .floor()
            .max(2.0) as usize;
        let longest = ((self.frames_per_second * 60.0 / TEMPO_MIN_BPM).ceil() as usize)
            .min(count / 2)
            .min(self.correlation.len() - 2);
        if shortest + 1 >= longest {
            return None;
        }
        for lag in shortest - 1..=longest + 1 {
            let overlap = count - lag;
            let sum: f32 = series[..overlap]
                .iter()
                .zip(&series[lag..])
                .map(|(a, b)| a * b)
                .sum();
            self.correlation[lag] = sum / (overlap as f32 * variance);
        }

        let correlation = &self.correlation;
        let is_peak = |lag: usize| {
            correlation[lag] > correlation[lag - 1] && correlation[lag] >= correlation[lag + 1]
        };
        let mut best = (shortest..=longest)
            .filter(|&lag| is_peak(lag))
            .max_by(|&a, &b| correlation[a].total_cmp(&correlation[b]).then(b.cmp(&a)))?;
        if correlation[best] < TEMPO_MIN_CORRELATION {
            return None;
        }
        // Prefer the beat to the bar: if half this period is nearly as strong, that is the beat.
        let half = best / 2;
        if let Some(beat) = (half.saturating_sub(1)..=half + 1)
            .filter(|&lag| lag >= shortest && is_peak(lag))
            .max_by(|&a, &b| correlation[a].total_cmp(&correlation[b]))
            && correlation[beat] >= TEMPO_HALF_PERIOD_RATIO * correlation[best]
        {
            best = beat;
        }

        // A parabola through the peak and its neighbours puts it between whole frames.
        let (before, at, after) = (
            correlation[best - 1],
            correlation[best],
            correlation[best + 1],
        );
        let curvature = before - 2.0 * at + after;
        let offset = if curvature < 0.0 {
            0.5 * (before - after) / curvature
        } else {
            0.0
        };
        Some(60.0 * self.frames_per_second / (best as f32 + offset.clamp(-0.5, 0.5)))
    }
}
