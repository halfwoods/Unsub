//! Rounding float samples to integers, with dither where the result would otherwise distort.

/// Quantizes samples in `-1.0..1.0` to signed integers of `bits` bits.
pub struct Quantizer {
    scale: f32,
    min: f32,
    max: f32,
    dither: bool,
    state: u64,
}

impl Quantizer {
    /// With `dither`, triangular noise of one step peak-to-peak either side is added before
    /// rounding. That turns the harmonic distortion of rounding a quiet signal into a steady, far
    /// less audible hiss. Fixed seed: the same input always gives the same output.
    pub fn new(bits: u32, dither: bool) -> Quantizer {
        let scale = (1u64 << (bits - 1)) as f32;
        Quantizer {
            scale,
            min: -scale,
            max: scale - 1.0,
            dither,
            state: 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub fn quantize(&mut self, sample: f32) -> i32 {
        let mut scaled = sample * self.scale;
        if self.dither {
            scaled += self.uniform() - self.uniform();
        }
        scaled.round().clamp(self.min, self.max) as i32
    }

    /// xorshift64*, mapped to `[0, 1)`.
    fn uniform(&mut self) -> f32 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let bits = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40;
        bits as f32 / 16_777_216.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_dither_every_16_bit_value_survives_a_round_trip() {
        let mut quantizer = Quantizer::new(16, false);
        for value in i16::MIN..=i16::MAX {
            assert_eq!(
                quantizer.quantize(f32::from(value) / 32_768.0),
                i32::from(value)
            );
        }
    }

    #[test]
    fn without_dither_24_bit_values_survive_a_round_trip() {
        let mut quantizer = Quantizer::new(24, false);
        for value in (-8_388_608..8_388_608).step_by(251) {
            assert_eq!(quantizer.quantize(value as f32 / 8_388_608.0), value);
        }
    }

    #[test]
    fn out_of_range_samples_clip_instead_of_wrapping() {
        let mut quantizer = Quantizer::new(16, true);
        assert_eq!(quantizer.quantize(2.0), 32_767);
        assert_eq!(quantizer.quantize(-2.0), -32_768);
        assert_eq!(quantizer.quantize(1.0), 32_767);
    }

    #[test]
    fn dither_noise_is_triangular_and_centred() {
        let mut quantizer = Quantizer::new(16, true);
        let noise: Vec<f32> = (0..200_000)
            .map(|_| quantizer.quantize(0.0) as f32)
            .collect();
        let mean = noise.iter().sum::<f32>() / noise.len() as f32;
        let variance = noise.iter().map(|n| (n - mean).powi(2)).sum::<f32>() / noise.len() as f32;
        assert!(mean.abs() < 0.01, "mean {mean}");
        // Triangular noise of +/-1 step has variance 1/6; rounding adds about 1/12.
        assert!((0.2..0.3).contains(&variance), "variance {variance}");
        assert!(
            noise.iter().all(|n| n.abs() <= 1.0),
            "never more than one step"
        );
    }

    #[test]
    fn dither_keeps_a_signal_below_one_step_instead_of_rounding_it_away() {
        // A constant 0.3 of a step: plain rounding makes it silence, dither preserves it on average.
        let level = 0.3 / 32_768.0;
        let mut plain = Quantizer::new(16, false);
        let mut dithered = Quantizer::new(16, true);
        let plain_mean: f32 = (0..100_000)
            .map(|_| plain.quantize(level) as f32)
            .sum::<f32>()
            / 1e5;
        let dithered_mean: f32 = (0..100_000)
            .map(|_| dithered.quantize(level) as f32)
            .sum::<f32>()
            / 1e5;
        assert_eq!(plain_mean, 0.0);
        assert!((dithered_mean - 0.3).abs() < 0.02, "{dithered_mean}");
    }

    #[test]
    fn the_same_input_always_gives_the_same_output() {
        let run = || {
            let mut quantizer = Quantizer::new(16, true);
            (0..1_000)
                .map(|n| quantizer.quantize(n as f32 / 4_000.0))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}
