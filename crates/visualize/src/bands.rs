//! Where the 64 bands sit on the FFT's bins, and how loud each is shown.

use crate::frame::BANDS;

const LOW_HZ: f64 = 30.0;
const HIGH_HZ: f64 = 16_000.0;
/// Levels at or below this many dB (relative to a full-scale sine) show as 0.
pub(crate) const FLOOR_DB: f32 = -70.0;
/// How much of the A-weighting curve is applied. All of it would bury the bass, which music
/// puts most of its energy in and which the visuals most need to show, under the mids.
const WEIGHTING: f64 = 0.5;

/// How a band reads the spectrum.
#[derive(Clone, Copy)]
enum Pick {
    /// The loudest bin inside the band, so a tone is as bright in a wide band as in a narrow one.
    Loudest { first: usize, last: usize },
    /// A band narrower than a bin falls between two of them: blend the pair.
    Between { lower: usize, mix: f32 },
}

pub(crate) struct BandLayout {
    picks: [Pick; BANDS],
    /// What the weighting adds to each band's level, in dB.
    weight_db: [f32; BANDS],
}

impl BandLayout {
    pub(crate) fn new(sample_rate: u32, fft_size: usize) -> Self {
        let nyquist = f64::from(sample_rate) / 2.0;
        let high = HIGH_HZ.min(nyquist);
        let ratio = (high / LOW_HZ).powf(1.0 / BANDS as f64);
        let edge = |index: usize| LOW_HZ * ratio.powi(index as i32);
        let bin_hz = f64::from(sample_rate) / fft_size as f64;
        let top_bin = fft_size / 2;

        let mut picks = [Pick::Loudest { first: 1, last: 1 }; BANDS];
        let mut weight_db = [0.0; BANDS];
        for (band, (pick, weight)) in picks.iter_mut().zip(&mut weight_db).enumerate() {
            let (low, high) = (edge(band), edge(band + 1));
            let first = (low / bin_hz).ceil() as usize;
            let last = ((high / bin_hz).ceil() as usize)
                .saturating_sub(1)
                .min(top_bin);
            *pick = if first <= last {
                Pick::Loudest { first, last }
            } else {
                let position = (low * high).sqrt() / bin_hz;
                let lower = (position.floor() as usize).clamp(1, top_bin - 1);
                Pick::Between {
                    lower,
                    mix: (position - lower as f64).clamp(0.0, 1.0) as f32,
                }
            };
            *weight = (WEIGHTING * a_weighting_db((low * high).sqrt())) as f32;
        }
        Self { picks, weight_db }
    }

    /// The level of each band in dB relative to a full-scale sine, from the bins' power.
    pub(crate) fn decibels(&self, power: &[f32], out: &mut [f32; BANDS]) {
        for (pick, db) in self.picks.iter().zip(out.iter_mut()) {
            let level = match *pick {
                Pick::Loudest { first, last } => power[first..=last]
                    .iter()
                    .fold(0.0_f32, |loudest, &bin| loudest.max(bin)),
                Pick::Between { lower, mix } => power[lower] * (1.0 - mix) + power[lower + 1] * mix,
            };
            *db = 10.0 * level.max(1e-12).log10();
        }
    }

    /// A band's level for the display: weighted, and mapped from -70..0 dB to 0..1.
    pub(crate) fn shown(&self, band: usize, db: f32) -> f32 {
        unit(db + self.weight_db[band])
    }
}

/// -70..0 dB as 0..1.
pub(crate) fn unit(db: f32) -> f32 {
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

/// The A-weighting curve (IEC 61672) in dB: 0 at 1 kHz.
fn a_weighting_db(hz: f64) -> f64 {
    let squared = hz * hz;
    let response = 12194.0_f64.powi(2) * squared * squared
        / ((squared + 20.6_f64.powi(2))
            * ((squared + 107.7_f64.powi(2)) * (squared + 737.9_f64.powi(2))).sqrt()
            * (squared + 12194.0_f64.powi(2)));
    20.0 * response.log10() + 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weighting_curve_matches_the_standard() {
        for (hz, expected) in [(100.0, -19.1), (1000.0, 0.0), (10_000.0, -2.5)] {
            assert!((a_weighting_db(hz) - expected).abs() < 0.1, "{hz} Hz");
        }
    }

    #[test]
    fn bands_cover_the_range_in_order_without_gaps_at_common_rates() {
        for rate in [22_050, 44_100, 48_000, 96_000, 192_000] {
            let layout = BandLayout::new(rate, 2048);
            let mut last_start = 0;
            for pick in layout.picks {
                let start = match pick {
                    Pick::Loudest { first, last } => {
                        assert!(first <= last && last <= 1024, "{rate}");
                        first
                    }
                    Pick::Between { lower, mix } => {
                        assert!((0.0..=1.0).contains(&mix) && lower < 1024, "{rate}");
                        lower
                    }
                };
                assert!(start >= last_start, "bands run low to high at {rate}");
                last_start = start;
            }
        }
    }
}
