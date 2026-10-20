//! The spectrum half of a frame: a windowed FFT of the mono mix, read as 64 log-spaced bands,
//! smoothed the way an analyzer's meters are, with peak caps, and the spectral flux the onset and
//! beat detectors work from.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use crate::bands::{BandLayout, unit};
use crate::frame::BANDS;
use crate::signal::Signal;

/// Bands rise to a new level with this time constant...
const ATTACK_SECONDS: f32 = 0.008;
/// ...and fall with this one, so a drop from full scale reaches 5% in about 300 ms.
const RELEASE_SECONDS: f32 = 0.1;
/// A hit in the bass is how far its level has risen above the average of the frames this long
/// before: a drum builds up over a few frames, so one frame's step would not show it, while
/// random flicker averages out.
const BASS_LOOKBACK_SECONDS: f32 = 0.045;
/// A band that is falling to silence is taken the rest of the way, so that silence is exactly 0
/// and the renderer can tell it from a very quiet sound.
const SNAP_TO_ZERO: f32 = 0.002;
/// A peak cap stays at its highest recent level this long, then falls back...
const PEAK_HOLD_SECONDS: f32 = 0.15;
/// ...at this many full scales a second.
const PEAK_FALL_PER_SECOND: f32 = 1.2;

/// How much the spectrum jumped since the previous frame: the mean over bands of the rises, in
/// the 0..1 display scale.
pub(crate) struct Flux {
    /// Over every band, as displayed (weighted): what the onset and tempo follow.
    pub(crate) overall: f32,
    /// The rise in the level of the bass as a whole, unweighted, above its recent average: what
    /// the beat detector follows.
    pub(crate) bass: f32,
}

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
    previous_shown: [f32; BANDS],
    /// The bass level of the last few frames, newest first.
    recent_bass: Vec<f32>,
    primed: bool,
}

impl Spectrum {
    pub(crate) fn new(sample_rate: u32, size: usize, hop: usize) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(size);
        let seconds_per_frame = hop as f32 / sample_rate as f32;
        let bass_lookback = (BASS_LOOKBACK_SECONDS / seconds_per_frame).round().max(2.0) as usize;
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
            previous_shown: [0.0; BANDS],
            recent_bass: vec![0.0; bass_lookback],
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
    /// their peak caps.
    pub(crate) fn analyze(
        &mut self,
        signal: &Signal,
        centre: u64,
        bands: &mut [f32; BANDS],
        peaks: &mut [f32; BANDS],
    ) -> Flux {
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
        let bass = unit(self.layout.bass_decibels(&self.power));
        if !self.primed {
            self.previous_shown = shown;
            self.recent_bass.fill(bass);
            self.smooth = shown;
            self.primed = true;
        }

        let overall = shown
            .iter()
            .zip(&self.previous_shown)
            .map(|(now, before)| (now - before).max(0.0))
            .sum::<f32>()
            / BANDS as f32;
        let flux = Flux {
            overall,
            bass: (bass - self.recent_bass.iter().sum::<f32>() / self.recent_bass.len() as f32)
                .max(0.0),
        };
        self.previous_shown = shown;
        self.recent_bass.rotate_right(1);
        self.recent_bass[0] = bass;

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
        flux
    }
}
