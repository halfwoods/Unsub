//! The ten-band graphic equalizer: its settings and presets, and the filters that apply them in
//! the audio callback.

use std::f64::consts::PI;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

pub const BANDS: usize = 10;

/// Band centres in Hz, an octave apart.
const CENTRES: [f64; BANDS] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

/// Roughly an octave wide, so each band meets its neighbours halfway.
const Q: f64 = 1.41;

/// How far a slider or the preamp can go either way.
const LIMIT_DB: f32 = 12.0;

/// A change of setting glides in this long: quick enough to follow a dragged slider, slow
/// enough that no change is a step in the waveform.
const RAMP_SECONDS: f64 = 0.020;

/// Bands above this share of the sample rate are left out: a filter centred near Nyquist
/// stops behaving like a bell.
const USABLE_SHARE_OF_RATE: f64 = 0.45;

/// The rates the filter gains are worked out at; see [`Equalizer::solved`]. A bell near the top
/// of the spectrum is shaped by the sample rate, so streams from 72 kHz up use the second set.
const DESIGN_RATES: [f64; 2] = [48_000.0, 96_000.0];
const DESIGN_SPLIT_HZ: f64 = 72_000.0;
const SOLVE_PASSES: usize = 12;

/// The lowest and highest frequencies the headroom check looks at.
const HEADROOM_SPAN_HZ: (f64, f64) = (20.0, 20_000.0);
const HEADROOM_POINTS: usize = 400;

/// Ten sliders and a preamp, as iTunes 10's Equalizer window has them. The response at each
/// band's centre is the slider's setting, however its neighbours are set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Equalizer {
    /// Applied before the bands, `-12.0..=12.0` dB.
    pub preamp_db: f32,
    /// Gains in dB, `-12.0..=12.0`, for 32, 64, 125, 250, 500 Hz and 1, 2, 4, 8, 16 kHz.
    pub bands: [f32; BANDS],
}

/// A named set of band gains.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub name: &'static str,
    pub bands: [f32; BANDS],
}

impl Equalizer {
    /// The preset called `name`, ignoring case, with no preamp.
    pub fn preset(name: &str) -> Option<Equalizer> {
        PRESETS
            .iter()
            .find(|preset| preset.name.eq_ignore_ascii_case(name))
            .map(|preset| Equalizer {
                preamp_db: 0.0,
                bands: preset.bands,
            })
    }

    /// This equalizer with the preamp lowered, if need be, until nothing it boosts can push a
    /// full-scale tone past full scale.
    ///
    /// Boosting makes a loud track clip, and clipping is distortion that no setting can undo.
    /// A soft clipper would hide it by distorting every loud peak; lowering the preamp costs
    /// level instead, stays linear, and leaves the choice with the listener, who turns the
    /// volume up to make it good again. The amount comes from the response of all the bands
    /// together, not from the largest slider, because neighbouring boosts add up between their
    /// centres. A preamp already lower than that is kept.
    pub fn with_headroom(self) -> Equalizer {
        let equalizer = self.limited();
        let solved = equalizer.solved(0);
        let (low, high) = HEADROOM_SPAN_HZ;
        let step = (high / low).ln() / (HEADROOM_POINTS - 1) as f64;
        let peak = (0..HEADROOM_POINTS)
            .map(|n| low * (step * n as f64).exp())
            .chain(CENTRES)
            .map(|frequency| response_db(&solved, frequency, DESIGN_RATES[0]))
            .fold(0.0, f64::max);
        Equalizer {
            preamp_db: equalizer.preamp_db.min(-(peak as f32)).max(-LIMIT_DB),
            ..equalizer
        }
    }

    /// The gain in dB at `frequency` for a stream at `rate`, preamp included, as computed from
    /// the filter design rather than measured.
    #[cfg(test)]
    pub(crate) fn response_db(&self, frequency: f64, rate: f64) -> f64 {
        let equalizer = self.limited();
        let set = usize::from(rate >= DESIGN_SPLIT_HZ);
        f64::from(equalizer.preamp_db) + response_db(&equalizer.solved(set), frequency, rate)
    }

    /// Settings held to what the sliders allow, with anything that is not a number as flat.
    fn limited(self) -> Equalizer {
        let limit = |gain: f32| {
            if gain.is_nan() {
                0.0
            } else {
                gain.clamp(-LIMIT_DB, LIMIT_DB)
            }
        };
        Equalizer {
            preamp_db: limit(self.preamp_db),
            bands: self.bands.map(limit),
        }
    }

    /// The gains to give the filters so that the response at each band's centre is the slider's
    /// setting. Bell filters overlap: two neighbours at +6 dB would make +8.5 dB between them,
    /// so each filter is set a little lower by what its neighbours add at its centre, found by
    /// repeating that correction until it settles. Worked out for design rate `set`; at rates
    /// between the two the centres land within a fraction of a decibel.
    fn solved(&self, set: usize) -> [f64; BANDS] {
        let target = self.bands.map(f64::from);
        let mut gains = target;
        if self.bands.iter().all(|&gain| gain == 0.0) {
            return gains;
        }
        for _ in 0..SOLVE_PASSES {
            for band in 0..BANDS {
                let reached = response_db(&gains, CENTRES[band], DESIGN_RATES[set]);
                gains[band] += target[band] - reached;
            }
        }
        gains
    }
}

/// Gain in dB of all the bands together at `frequency`.
fn response_db(gains: &[f64; BANDS], frequency: f64, rate: f64) -> f64 {
    gains
        .iter()
        .enumerate()
        .map(|(band, &gain)| magnitude_db(&coefficients(band, gain, rate), frequency, rate))
        .sum()
}

/// A biquad, `a0` divided out.
#[derive(Clone, Copy)]
struct Coefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

/// The "peaking EQ" of the RBJ Audio EQ Cookbook for one band.
fn coefficients(band: usize, gain_db: f64, rate: f64) -> Coefficients {
    let w0 = 2.0 * PI * CENTRES[band] / rate;
    bell(w0.cos(), w0.sin() / (2.0 * Q), gain_db)
}

impl Coefficients {
    /// Leaves the signal exactly as it is.
    const PASS: Coefficients = Coefficients {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };
}

fn bell(cos_w0: f64, alpha: f64, gain_db: f64) -> Coefficients {
    let a = 10f64.powf(gain_db / 40.0);
    let a0 = 1.0 + alpha / a;
    Coefficients {
        b0: (1.0 + alpha * a) / a0,
        b1: -2.0 * cos_w0 / a0,
        b2: (1.0 - alpha * a) / a0,
        a1: -2.0 * cos_w0 / a0,
        a2: (1.0 - alpha / a) / a0,
    }
}

fn magnitude_db(c: &Coefficients, frequency: f64, rate: f64) -> f64 {
    let w = 2.0 * PI * frequency / rate;
    let (cos1, sin1, cos2, sin2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let numerator =
        (c.b0 + c.b1 * cos1 + c.b2 * cos2).powi(2) + (c.b1 * sin1 + c.b2 * sin2).powi(2);
    let denominator =
        (1.0 + c.a1 * cos1 + c.a2 * cos2).powi(2) + (c.a1 * sin1 + c.a2 * sin2).powi(2);
    10.0 * (numerator / denominator).log10()
}

/// The equalizer as the user last set it, shared lock-free between the player and the audio
/// callback. A setting is applied as a whole a moment later, so a reader that catches it half
/// written only glides toward a mixture of old and new for the length of one buffer.
#[derive(Default)]
pub struct EqControl {
    on: AtomicBool,
    /// Per design rate: preamp, then the filter gains of the bands, as `f32` bits.
    gains: [[AtomicU32; BANDS + 1]; DESIGN_RATES.len()],
}

impl EqControl {
    /// `None` turns the equalizer off.
    pub fn set(&self, equalizer: Option<Equalizer>) {
        let Some(equalizer) = equalizer else {
            self.on.store(false, Ordering::Release);
            return;
        };
        let equalizer = equalizer.limited();
        for (set, gains) in self.gains.iter().enumerate() {
            gains[0].store(equalizer.preamp_db.to_bits(), Ordering::Relaxed);
            for (slot, gain) in gains[1..].iter().zip(equalizer.solved(set)) {
                slot.store((gain as f32).to_bits(), Ordering::Relaxed);
            }
        }
        self.on.store(true, Ordering::Release);
    }

    /// Whether an equalizer is set, flat or not.
    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    /// Where the filters should be for design rate `set`: flat unless an equalizer is set.
    fn targets(&self, set: usize) -> [f64; BANDS + 1] {
        let mut targets = [0.0; BANDS + 1];
        if self.is_on() {
            for (target, slot) in targets.iter_mut().zip(&self.gains[set]) {
                *target = f64::from(f32::from_bits(slot.load(Ordering::Relaxed)));
            }
        }
        targets
    }
}

#[derive(Clone, Copy, Default)]
struct History {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

/// The real-time half: one cascade of bells per channel, gliding to whatever [`EqControl`] asks
/// for. Allocates only when built, and does nothing at all while the equalizer is flat or off,
/// so that case passes samples through bit for bit.
pub struct Chain {
    control: Arc<EqControl>,
    /// Which of the control's gain sets suits this stream's rate.
    set: usize,
    ramp_frames: u32,
    cos_w0: [f64; BANDS],
    alpha: [f64; BANDS],
    usable: [bool; BANDS],
    /// Preamp then bands, in dB: where each is, where it is going, and how far it moves a frame.
    current: [f64; BANDS + 1],
    target: [f64; BANDS + 1],
    step: [f64; BANDS + 1],
    remaining: u32,
    coefficients: [Coefficients; BANDS],
    preamp: f64,
    histories: Vec<[History; BANDS]>,
    active: bool,
}

impl Chain {
    /// Starts where `control` is now, with no glide.
    pub fn new(control: Arc<EqControl>, sample_rate: u32, channels: usize) -> Chain {
        let rate = f64::from(sample_rate);
        let w0 = CENTRES.map(|centre| 2.0 * PI * centre / rate);
        let set = usize::from(rate >= DESIGN_SPLIT_HZ);
        let target = control.targets(set);
        let mut chain = Chain {
            control,
            set,
            ramp_frames: ((rate * RAMP_SECONDS).round() as u32).max(1),
            cos_w0: w0.map(f64::cos),
            alpha: w0.map(|w0| w0.sin() / (2.0 * Q)),
            usable: CENTRES.map(|centre| centre < rate * USABLE_SHARE_OF_RATE),
            current: target,
            target,
            step: [0.0; BANDS + 1],
            remaining: 0,
            coefficients: [Coefficients::PASS; BANDS],
            preamp: 1.0,
            histories: vec![[History::default(); BANDS]; channels],
            active: target.iter().any(|&gain| gain != 0.0),
        };
        chain.recompute(&[true; BANDS + 1]);
        chain
    }

    /// Picks up a change of setting; call once per buffer.
    pub fn sync(&mut self) {
        let target = self.control.targets(self.set);
        if target == self.target {
            return;
        }
        self.target = target;
        self.remaining = self.ramp_frames;
        for ((step, &to), &from) in self.step.iter_mut().zip(&target).zip(&self.current) {
            *step = (to - from) / f64::from(self.ramp_frames);
        }
        if !self.active {
            // Flat means the filters are the identity, so they can start from nothing and be
            // right within two samples.
            self.reset();
            self.active = true;
        }
    }

    /// Moves the glide on by one frame.
    pub fn advance(&mut self) {
        if self.remaining == 0 {
            return;
        }
        self.remaining -= 1;
        let moving = self.step.map(|step| step != 0.0);
        if self.remaining == 0 {
            self.current = self.target;
            self.step = [0.0; BANDS + 1];
        } else {
            for (gain, step) in self.current.iter_mut().zip(&self.step) {
                *gain += step;
            }
        }
        self.recompute(&moving);
        if self.remaining == 0 && self.current.iter().all(|&gain| gain == 0.0) {
            self.active = false;
        }
    }

    pub fn process(&mut self, channel: usize, sample: f32) -> f32 {
        if !self.active {
            return sample;
        }
        let mut value = f64::from(sample) * self.preamp;
        for (c, h) in self.coefficients.iter().zip(&mut self.histories[channel]) {
            let out = c.b0 * value + c.b1 * h.x1 + c.b2 * h.x2 - c.a1 * h.y1 - c.a2 * h.y2;
            *h = History {
                x1: value,
                x2: h.x1,
                y1: out,
                y2: h.y1,
            };
            value = out;
        }
        value as f32
    }

    /// Forgets the signal, for when the audio it was filtering is discarded.
    pub fn reset(&mut self) {
        for channel in &mut self.histories {
            *channel = [History::default(); BANDS];
        }
    }

    fn recompute(&mut self, moving: &[bool; BANDS + 1]) {
        if moving[0] {
            self.preamp = 10f64.powf(self.current[0] / 20.0);
        }
        for band in 0..BANDS {
            // Past Nyquist the bell's poles fall outside the unit circle, where even a flat
            // setting would blow up on rounding error, so such a band is skipped outright.
            if moving[band + 1] && self.usable[band] {
                self.coefficients[band] =
                    bell(self.cos_w0[band], self.alpha[band], self.current[band + 1]);
            }
        }
    }
}

const fn preset(name: &'static str, bands: [f32; BANDS]) -> Preset {
    Preset { name, bands }
}

/// Starting points for the sliders, in dB at the band centres.
#[rustfmt::skip]
pub const PRESETS: &[Preset] = &[
    //                          32     64    125    250    500     1k     2k     4k     8k    16k
    preset("Acoustic", [3.0, 3.0, 2.0, 1.0, 1.0, 0.0, 1.0, 2.0, 3.0, 2.0]),
    preset("Bass Booster", [6.0, 5.0, 4.0, 2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    preset("Bass Reducer", [-6.0, -5.0, -4.0, -2.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    preset("Classical", [2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 2.0, 3.0]),
    preset("Dance", [5.0, 6.0, 3.0, 0.0, 0.0, -3.0, -2.0, 0.0, 3.0, 4.0]),
    preset("Deep", [5.0, 4.0, 3.0, 1.0, 0.0, -1.0, -2.0, -3.0, -4.0, -5.0]),
    preset("Electronic", [4.0, 3.0, 1.0, 0.0, -2.0, 2.0, 1.0, 2.0, 4.0, 4.0]),
    preset("Hip-Hop", [5.0, 5.0, 3.0, 3.0, 1.0, -1.0, 1.0, 0.0, 2.0, 3.0]),
    preset("Jazz", [3.0, 2.0, 1.0, 2.0, -2.0, -2.0, 0.0, 1.0, 2.0, 3.0]),
    preset("Latin", [4.0, 2.0, 0.0, 0.0, -2.0, 0.0, 1.0, 3.0, 4.0, 4.0]),
    preset("Loudness", [6.0, 4.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0, 4.0, 5.0]),
    preset("Lounge", [-2.0, -1.0, 0.0, 2.0, 3.0, 2.0, 1.0, 0.0, -2.0, -3.0]),
    preset("Piano", [1.0, 1.0, 0.0, 1.0, 2.0, 1.0, 3.0, 4.0, 3.0, 2.0]),
    preset("Pop", [-1.0, 1.0, 3.0, 4.0, 3.0, 0.0, -1.0, -1.0, 1.0, 2.0]),
    preset("R&B", [4.0, 6.0, 4.0, 1.0, -2.0, -1.0, 2.0, 2.0, 2.0, 3.0]),
    preset("Rock", [5.0, 4.0, 2.0, -2.0, -3.0, -1.0, 2.0, 4.0, 5.0, 5.0]),
    preset("Small Speakers", [5.0, 5.0, 4.0, 3.0, 1.0, 0.0, -1.0, -2.0, -3.0, -4.0]),
    preset("Spoken Word", [-4.0, -3.0, 0.0, 1.0, 3.0, 4.0, 4.0, 3.0, 1.0, 0.0]),
    preset("Treble Booster", [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 3.0, 4.0, 5.0, 6.0]),
    preset("Treble Reducer", [0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -3.0, -4.0, -5.0, -6.0]),
    preset("Vocal Booster", [-2.0, -3.0, -3.0, 1.0, 3.0, 4.0, 4.0, 3.0, 1.0, 0.0]),
];

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: [u32; 3] = [44_100, 48_000, 96_000];

    fn control(equalizer: Option<Equalizer>) -> Arc<EqControl> {
        let control = Arc::new(EqControl::default());
        control.set(equalizer);
        control
    }

    /// Runs a mono sine through a chain for `seconds`, one frame at a time as the callback does.
    fn sine_through(
        chain: &mut Chain,
        rate: u32,
        frequency: f64,
        amplitude: f64,
        seconds: f64,
    ) -> Vec<f32> {
        (0..(f64::from(rate) * seconds) as usize)
            .map(|n| {
                chain.sync();
                chain.advance();
                let phase = 2.0 * PI * frequency * n as f64 / f64::from(rate);
                chain.process(0, (amplitude * phase.sin()) as f32)
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f64 {
        (samples.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
    }

    /// The gain a chain gives a steady sine, measured on its output once it has settled.
    fn measured_db(equalizer: Equalizer, rate: u32, frequency: f64) -> f64 {
        let mut chain = Chain::new(control(Some(equalizer)), rate, 1);
        let amplitude = 0.1;
        let out = sine_through(&mut chain, rate, frequency, amplitude, 1.0);
        let settled = &out[out.len() / 2..];
        20.0 * (rms(settled) / (amplitude / 2f64.sqrt())).log10()
    }

    #[test]
    fn presets_are_distinct_named_and_within_the_sliders_range() {
        assert_eq!(PRESETS.len(), 21);
        for (n, preset) in PRESETS.iter().enumerate() {
            assert!(
                preset.bands.iter().all(|gain| gain.abs() <= LIMIT_DB),
                "{}",
                preset.name
            );
            assert!(
                preset.bands.iter().any(|&gain| gain != 0.0),
                "{}",
                preset.name
            );
            assert!(
                PRESETS[n + 1..]
                    .iter()
                    .all(|other| other.name != preset.name && other.bands != preset.bands),
                "{} repeats another preset",
                preset.name
            );
        }
        assert_eq!(
            Equalizer::preset("bass booster").map(|e| e.bands[0]),
            Some(6.0)
        );
        assert_eq!(Equalizer::preset("No Such Preset"), None);
    }

    #[test]
    fn every_band_centre_reaches_its_setting_whatever_the_neighbours_do() {
        let zigzag = Equalizer {
            preamp_db: 0.0,
            bands: [
                12.0, -12.0, 12.0, -12.0, 12.0, -12.0, 12.0, -12.0, 12.0, -12.0,
            ],
        };
        let presets = PRESETS.iter().map(|preset| {
            (
                preset.name,
                Equalizer {
                    preamp_db: 0.0,
                    bands: preset.bands,
                },
            )
        });
        // The zigzag is the hardest case for overlapping bells: every slider against its
        // neighbours.
        for (name, equalizer) in presets.chain([("zigzag", zigzag)]) {
            for rate in RATES {
                for (band, &gain) in equalizer.bands.iter().enumerate() {
                    let got = equalizer.response_db(CENTRES[band], f64::from(rate));
                    assert!(
                        (got - f64::from(gain)).abs() < 0.5,
                        "{name} band {band} at {rate} Hz: {got} dB, set {gain} dB"
                    );
                }
            }
        }
    }

    #[test]
    fn the_filters_themselves_give_the_response_at_band_centres() {
        for rate in RATES {
            for name in ["Rock", "Bass Booster", "Spoken Word"] {
                let mut equalizer = Equalizer::preset(name).unwrap();
                equalizer.preamp_db = -2.0;
                for (band, &gain) in equalizer.bands.iter().enumerate() {
                    let got = measured_db(equalizer, rate, CENTRES[band]);
                    let want = f64::from(gain) - 2.0;
                    assert!(
                        (got - want).abs() < 0.5,
                        "{name} band {band} at {rate} Hz: {got}, want {want}"
                    );
                }
            }
        }
    }

    #[test]
    fn between_the_centres_the_filters_follow_the_computed_response() {
        let equalizer = Equalizer::preset("Rock").unwrap();
        for frequency in [45.0, 300.0, 700.0, 3_000.0, 11_000.0] {
            let got = measured_db(equalizer, 48_000, frequency);
            let want = equalizer.response_db(frequency, 48_000.0);
            assert!((got - want).abs() < 0.15, "{frequency} Hz: {got} vs {want}");
        }
    }

    #[test]
    fn flat_or_off_passes_samples_through_bit_for_bit() {
        let samples: Vec<f32> = (0..2_000)
            .map(|n| ((n * 7919) % 2_001) as f32 / 1_000.0 - 1.0)
            .collect();
        for setting in [None, Some(Equalizer::default())] {
            let mut chain = Chain::new(control(setting), 44_100, 2);
            for (n, &sample) in samples.iter().enumerate() {
                chain.sync();
                chain.advance();
                assert_eq!(chain.process(n % 2, sample).to_bits(), sample.to_bits());
            }
        }
    }

    #[test]
    fn switching_off_glides_down_and_then_passes_bit_for_bit_again() {
        let control = control(Some(Equalizer::preset("Rock").unwrap()));
        let mut chain = Chain::new(Arc::clone(&control), 48_000, 1);
        sine_through(&mut chain, 48_000, 1_000.0, 0.1, 0.1);
        assert!(chain.active);

        control.set(None);
        sine_through(&mut chain, 48_000, 1_000.0, 0.1, 0.1);
        assert!(!chain.active, "once flat the filters are bypassed");
        assert_eq!(chain.process(0, 0.25).to_bits(), 0.25f32.to_bits());
    }

    /// The largest step between neighbouring samples of `out`.
    fn largest_step(out: &[f32]) -> f32 {
        out.windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    }

    /// Plays a 1 kHz sine through a fresh chain, applying `setting` on the sine's crest.
    fn sine_with_change(setting: Equalizer, ramp_frames: Option<u32>) -> Vec<f32> {
        let rate = 48_000;
        let control = control(None);
        let mut chain = Chain::new(Arc::clone(&control), rate, 1);
        if let Some(frames) = ramp_frames {
            chain.ramp_frames = frames;
        }
        // 48 samples a period, so frame 2412 is a crest, where a step in gain would be largest.
        let change_at = 2_412;
        (0..9_600)
            .map(|n| {
                if n == change_at {
                    control.set(Some(setting));
                }
                chain.sync();
                chain.advance();
                let phase = 2.0 * PI * 1_000.0 * n as f64 / f64::from(rate);
                chain.process(0, (0.1 * phase.sin()) as f32)
            })
            .collect()
    }

    #[test]
    fn a_live_change_glides_instead_of_clicking() {
        // +12 dB quadruples the tone, and a sine's own steepest step scales with it.
        let steepest_sine = 0.4 * 2.0 * std::f32::consts::PI * 1_000.0 / 48_000.0;
        let band = Equalizer {
            preamp_db: 0.0,
            bands: [0.0, 0.0, 0.0, 0.0, 0.0, 12.0, 0.0, 0.0, 0.0, 0.0],
        };
        let preamp = Equalizer {
            preamp_db: 12.0,
            bands: [0.0; BANDS],
        };
        for setting in [band, preamp] {
            let glided = largest_step(&sine_with_change(setting, None));
            assert!(
                glided < steepest_sine * 1.1,
                "{glided} vs the tone's own {steepest_sine}"
            );
        }
        // Without the glide the preamp's change is a step in the waveform, so the bound can fail.
        assert!(largest_step(&sine_with_change(preamp, Some(1))) > steepest_sine * 2.0);
    }

    #[test]
    fn a_change_moves_every_setting_in_equal_steps_and_lands_exactly() {
        let control = control(None);
        let mut chain = Chain::new(Arc::clone(&control), 48_000, 1);
        chain.sync();
        let ramp = chain.ramp_frames;
        control.set(Some(Equalizer::preset("Bass Booster").unwrap()));
        let before = chain.current;
        chain.sync();
        let mut previous = before;
        for frame in 1..=ramp {
            chain.advance();
            let moved: Vec<f64> = chain
                .current
                .iter()
                .zip(&previous)
                .map(|(a, b)| a - b)
                .collect();
            let expected = (chain.target[1] - before[1]) / f64::from(ramp);
            assert!((moved[1] - expected).abs() < 1e-9, "frame {frame}");
            previous = chain.current;
        }
        assert_eq!(
            chain.current, chain.target,
            "lands on the target, not near it"
        );
    }

    #[test]
    fn the_preamp_alone_scales_the_signal() {
        let equalizer = Equalizer {
            preamp_db: -6.0,
            bands: [0.0; BANDS],
        };
        let got = measured_db(equalizer, 48_000, 440.0);
        assert!((got + 6.0).abs() < 0.01, "{got}");
    }

    #[test]
    fn headroom_keeps_every_preset_under_full_scale_and_leaves_flat_and_cuts_alone() {
        let (low, high) = HEADROOM_SPAN_HZ;
        for preset in PRESETS {
            let equalizer = Equalizer {
                preamp_db: 0.0,
                bands: preset.bands,
            }
            .with_headroom();
            assert!(equalizer.preamp_db <= 0.0, "{}", preset.name);
            let peak = (0..600)
                .map(|n| low * (high / low).powf(f64::from(n) / 599.0))
                .map(|frequency| equalizer.response_db(frequency, DESIGN_RATES[0]))
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(peak < 0.05, "{}: {peak} dB", preset.name);
        }
        assert_eq!(Equalizer::default().with_headroom(), Equalizer::default());
        let cut = Equalizer::preset("Bass Reducer").unwrap();
        // Only the small boosts that make up for a cut's reach into its neighbours need room.
        assert!(
            cut.with_headroom().preamp_db > -0.1,
            "cuts need next to no headroom"
        );
        let lower = Equalizer {
            preamp_db: -12.0,
            ..Equalizer::preset("Rock").unwrap()
        };
        assert_eq!(
            lower.with_headroom().preamp_db,
            -12.0,
            "a lower preamp is kept"
        );
    }

    #[test]
    fn headroom_counts_what_neighbouring_boosts_add_between_them() {
        let bands = [0.0, 0.0, 0.0, 0.0, 6.0, 6.0, 6.0, 0.0, 0.0, 0.0];
        let preamp = Equalizer {
            preamp_db: 0.0,
            bands,
        }
        .with_headroom()
        .preamp_db;
        assert!(preamp < -6.0, "more than the largest slider: {preamp}");
    }

    #[test]
    fn settings_beyond_the_sliders_or_not_numbers_are_held_in_range() {
        let wild = Equalizer {
            preamp_db: f32::NAN,
            bands: [
                40.0,
                -40.0,
                f32::NAN,
                f32::INFINITY,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
            ],
        };
        let limited = wild.limited();
        assert_eq!(limited.preamp_db, 0.0);
        assert_eq!(&limited.bands[..4], &[12.0, -12.0, 0.0, 12.0]);
        let control = control(Some(wild));
        assert!(control.targets(0).iter().all(|gain| gain.is_finite()));
    }

    #[test]
    fn the_settings_read_and_write_camel_case_json() {
        let equalizer = Equalizer {
            preamp_db: -3.0,
            bands: [1.0; BANDS],
        };
        let json = serde_json::to_value(equalizer).unwrap();
        assert_eq!(json["preampDb"], -3.0);
        assert_eq!(
            serde_json::from_value::<Equalizer>(json).unwrap(),
            equalizer
        );
    }

    #[test]
    fn bands_too_close_to_the_nyquist_rate_are_left_out() {
        let top_only = Equalizer {
            preamp_db: 0.0,
            bands: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 12.0],
        };
        // 16 kHz is above Nyquist at 22.05 kHz, where no bell can be built, and must not
        // disturb what can be heard.
        let elsewhere = measured_db(top_only, 22_050, 1_000.0);
        assert!(elsewhere.abs() < 0.1, "{elsewhere}");
    }
}
