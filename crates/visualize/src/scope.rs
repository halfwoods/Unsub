//! The waveform snapshot: a short stretch of each channel, thinned to 256 points, started where
//! the signal rises through zero so that a steady tone is drawn in the same place every frame.

use crate::frame::SCOPE_POINTS;
use crate::signal::Signal;

/// About this many points per second after thinning; at 44.1 kHz a frame shows 23 ms.
const TARGET_RATE: f64 = 11_025.0;
/// How far back from the ideal start to look for a rising crossing, in seconds. It covers one
/// cycle of the lowest bass the bands show.
const SEARCH_SECONDS: f64 = 0.035;

pub(crate) struct Scope {
    /// Raw samples averaged into each point.
    factor: usize,
    search_back: u64,
}

impl Scope {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let factor = (f64::from(sample_rate) / TARGET_RATE).round().max(1.0) as usize;
        Self {
            factor,
            search_back: (SEARCH_SECONDS * f64::from(sample_rate)) as u64,
        }
    }

    /// Samples shown, before the start moves to a crossing.
    pub(crate) fn span(&self) -> u64 {
        (SCOPE_POINTS * self.factor) as u64
    }

    /// Fills both waveforms for the moment `centre`. Needs the samples up to `centre + span / 2`.
    pub(crate) fn capture(
        &self,
        signal: &Signal,
        centre: u64,
        left: &mut [f32; SCOPE_POINTS],
        right: &mut [f32; SCOPE_POINTS],
    ) {
        let ideal = centre
            .saturating_sub(self.span() / 2)
            .max(signal.segment_start());
        let start = self.rising_crossing(signal, ideal).unwrap_or(ideal);
        let span = SCOPE_POINTS * self.factor;
        let scale = 1.0 / self.factor as f32;
        let thin = |samples: &[f32], out: &mut [f32; SCOPE_POINTS]| {
            for (point, group) in out.iter_mut().zip(samples.chunks_exact(self.factor)) {
                *point = (group.iter().sum::<f32>() * scale).clamp(-1.0, 1.0);
            }
        };
        thin(signal.left(start, span), left);
        thin(signal.right(start, span), right);
    }

    /// The latest point at or before `ideal` where the signal, smoothed over one point's width,
    /// goes from below zero to zero or above.
    fn rising_crossing(&self, signal: &Signal, ideal: u64) -> Option<u64> {
        let oldest = signal
            .segment_start()
            .max(ideal.saturating_sub(self.search_back))
            + 1;
        let width = self.factor;
        // The samples from just before the earliest position to the end of the latest window.
        let first = oldest - 1;
        let samples = signal.mono(first, (ideal - first) as usize + width);
        let mut position = (ideal - first) as usize;
        let mut after: f32 = samples[position..position + width].iter().sum();
        while position >= 1 {
            // Slide the window back one sample: the average over `position - 1 ..`.
            let before = after - samples[position + width - 1] + samples[position - 1];
            if before < 0.0 && after >= 0.0 {
                return Some(first + position as u64);
            }
            after = before;
            position -= 1;
        }
        None
    }
}
