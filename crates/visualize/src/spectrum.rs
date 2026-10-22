//! The spectrum half of a frame: a windowed FFT of the mono mix, read as 64 log-spaced bands,
//! smoothed the way an analyzer's meters are, with peak caps, and the accent (spectral flux) the
//! onset, beat and tempo detectors work from.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use crate::bands::BandLayout;
use crate::frame::BANDS;
use crate::signal::Signal;

/// Bands rise to a new level with this time constant...
const ATTACK_SECONDS: f32 = 0.008;
/// ...and fall with this one, so a drop from full scale reaches 5% in about 300 ms.
const RELEASE_SECONDS: f32 = 0.1;
/// A band has risen as far as it stands above its average over this many frames before: a drum
/// builds up over a few frames, so one frame's step would not show it, while the flicker of a
/// steady sound averages out.
const ACCENT_LOOKBACK_FRAMES: usize = 3;
/// A band that is falling to silence is taken the rest of the way, so that silence is exactly 0
/// and the renderer can tell it from a very quiet sound.
const SNAP_TO_ZERO: f32 = 0.002;
/// A peak cap stays at its highest recent level this long, then falls back...
const PEAK_HOLD_SECONDS: f32 = 0.15;
/// ...at this many full scales a second.
const PEAK_FALL_PER_SECOND: f32 = 1.2;

pub(crate) struct Spectrum {
    size: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Power per bin, scaled so a full-scale sine centred on a bin reads 1.
    power: Vec<f32>,
    layout: BandLayout,
    attack: f32,
    release: f32,
    seconds_per_frame: f32,
    smooth: [f32; BANDS],
    peaks: [f32; BANDS],
    peak_hold: [f32; BANDS],
    /// The last few frames' levels as displayed, newest first.
    recent_shown: [[f32; BANDS]; ACCENT_LOOKBACK_FRAMES],
    primed: bool,
}

impl Spectrum {
    pub(crate) fn new(sample_rate: u32, size: usize, hop: usize) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(size);
        let seconds_per_frame = hop as f32 / sample_rate as f32;
        // Periodic Hann: sums to size / 2, so a sine of amplitude A peaks at A * size / 4.
        let window = (0..size)
            .map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / size as f32).cos())
            .collect();
        Self {
            size,
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            window,
            power: vec![0.0; size / 2 + 1],
            layout: BandLayout::new(sample_rate, size),
            attack: 1.0 - (-seconds_per_frame / ATTACK_SECONDS).exp(),
            release: 1.0 - (-seconds_per_frame / RELEASE_SECONDS).exp(),
            seconds_per_frame,
            smooth: [0.0; BANDS],
            peaks: [0.0; BANDS],
            peak_hold: [0.0; BANDS],
            recent_shown: [[0.0; BANDS]; ACCENT_LOOKBACK_FRAMES],
            primed: false,
        }
    }

    /// Forgets the meters, as at the start of a new song.
    pub(crate) fn reset(&mut self) {
        self.smooth = [0.0; BANDS];
        self.peaks = [0.0; BANDS];
        self.peak_hold = [0.0; BANDS];
        self.primed = false;
    }

    /// Analyzes the window of `size` samples centred on `centre`, writing the smoothed bands and
    /// their peak caps. Returns the accent: how much the spectrum jumped, as the mean over bands of
    /// the rise above their recent average, in the 0..1 display scale.
    pub(crate) fn analyze(
        &mut self,
        signal: &Signal,
        centre: u64,
        bands: &mut [f32; BANDS],
        peaks: &mut [f32; BANDS],
    ) -> f32 {
        let samples = signal.mono(centre - (self.size / 2) as u64, self.size);
        for ((input, &sample), &weight) in self.input.iter_mut().zip(samples).zip(&self.window) {
            *input = sample * weight;
        }
        // The input and output are the right sizes by construction, so this cannot fail.
        let _ = self
            .fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch);
        let scale = 16.0 / (self.size * self.size) as f32;
        for (power, bin) in self.power.iter_mut().zip(&self.output) {
            *power = bin.norm_sqr() * scale;
        }

        let mut decibels = [0.0; BANDS];
        self.layout.decibels(&self.power, &mut decibels);
        let mut shown = [0.0; BANDS];
        for (band, &db) in decibels.iter().enumerate() {
            shown[band] = self.layout.shown(band, db);
        }
        if !self.primed {
            self.recent_shown = [shown; ACCENT_LOOKBACK_FRAMES];
            self.smooth = shown;
            self.primed = true;
        }

        let accent = shown
            .iter()
            .enumerate()
            .map(|(band, level)| {
                let before = self
                    .recent_shown
                    .iter()
                    .map(|frame| frame[band])
                    .sum::<f32>()
                    / ACCENT_LOOKBACK_FRAMES as f32;
                (level - before).max(0.0)
            })
            .sum::<f32>()
            / BANDS as f32;
        self.recent_shown.rotate_right(1);
        self.recent_shown[0] = shown;

        let meters = self
            .smooth
            .iter_mut()
            .zip(&mut self.peaks)
            .zip(&mut self.peak_hold)
            .zip(&shown);
        for (((smooth, peak), hold), &level) in meters {
            let coefficient = if level > *smooth {
                self.attack
            } else {
                self.release
            };
            *smooth += (level - *smooth) * coefficient;
            if level == 0.0 && *smooth < SNAP_TO_ZERO {
                *smooth = 0.0;
            }
            if *smooth >= *peak {
                *peak = *smooth;
                *hold = PEAK_HOLD_SECONDS;
            } else if *hold > 0.0 {
                *hold -= self.seconds_per_frame;
            } else {
                *peak = (*peak - PEAK_FALL_PER_SECOND * self.seconds_per_frame).max(*smooth);
            }
        }
        *bands = self.smooth;
        *peaks = self.peaks;
        accent
    }
}
