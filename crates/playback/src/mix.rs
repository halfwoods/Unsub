//! Speaker layouts and the channel mixing between a source and the device.

use std::f32::consts::FRAC_1_SQRT_2;

// Speaker bits from WAVEFORMATEXTENSIBLE, which symphonia's `Position` and FFmpeg's `AV_CH_*`
// both follow for the common channels.
const FRONT_LEFT: u64 = 1 << 0;
const FRONT_RIGHT: u64 = 1 << 1;
const FRONT_CENTER: u64 = 1 << 2;
const LFE: u64 = 1 << 3;
const REAR_LEFT: u64 = 1 << 4;
const REAR_RIGHT: u64 = 1 << 5;
const SIDE_LEFT: u64 = 1 << 9;
const SIDE_RIGHT: u64 = 1 << 10;

const FRONT: u64 = FRONT_LEFT | FRONT_RIGHT | FRONT_CENTER | LFE;
const REAR: u64 = REAR_LEFT | REAR_RIGHT;
const SIDE: u64 = SIDE_LEFT | SIDE_RIGHT;

/// What a decoded stream's channels are, in the order decoders emit them (low speaker bit first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelLayout {
    Mono,
    Stereo,
    /// Front left and right, centre, LFE and one surround pair (rear or side).
    Surround5_1,
    /// 5.1 with both a rear and a side surround pair.
    Surround7_1,
    /// Any other arrangement, known only by its channel count.
    Other(u16),
}

impl ChannelLayout {
    /// `mask` is the speaker bit mask, zero when the decoder does not say which speakers.
    pub fn from_mask(mask: u64, channels: u16) -> Self {
        let has = |bits: u64| mask & bits == bits;
        match channels {
            1 => ChannelLayout::Mono,
            2 => ChannelLayout::Stereo,
            6 if has(FRONT) && (has(REAR) || has(SIDE)) => ChannelLayout::Surround5_1,
            8 if has(FRONT | REAR | SIDE) => ChannelLayout::Surround7_1,
            _ => ChannelLayout::Other(channels),
        }
    }

    pub fn channels(self) -> u16 {
        match self {
            ChannelLayout::Mono => 1,
            ChannelLayout::Stereo => 2,
            ChannelLayout::Surround5_1 => 6,
            ChannelLayout::Surround7_1 => 8,
            ChannelLayout::Other(channels) => channels,
        }
    }
}

/// ITU-R BS.775 stereo downmix weights for the centre and each surround channel.
const CENTER: f32 = FRAC_1_SQRT_2;
const SURROUND: f32 = FRAC_1_SQRT_2;

/// Weights of each source channel in the left output of a downmix, then the right output.
/// LFE is left out, as BS.775 does. A 7.1 source has no BS.775 form, so its two surround pairs
/// share the single BS.775 surround weight, at -3 dB each.
const DOWNMIX_5_1: [[f32; 6]; 2] = [
    [1.0, 0.0, CENTER, 0.0, SURROUND, 0.0],
    [0.0, 1.0, CENTER, 0.0, 0.0, SURROUND],
];
const DOWNMIX_7_1: [[f32; 8]; 2] = [
    [
        1.0,
        0.0,
        CENTER,
        0.0,
        SURROUND * FRAC_1_SQRT_2,
        0.0,
        SURROUND * FRAC_1_SQRT_2,
        0.0,
    ],
    [
        0.0,
        1.0,
        CENTER,
        0.0,
        0.0,
        SURROUND * FRAC_1_SQRT_2,
        0.0,
        SURROUND * FRAC_1_SQRT_2,
    ],
];

/// Converts interleaved frames from a source layout to a device channel count.
pub struct Mixer {
    from: usize,
    to: usize,
    /// Row-major `to` x `from` weights; `None` when the channels pass straight through.
    matrix: Option<Vec<f32>>,
}

impl Mixer {
    pub fn new(layout: ChannelLayout, to: u16) -> Mixer {
        let from = usize::from(layout.channels());
        let to = usize::from(to);
        let matrix = match (layout, to) {
            _ if from == to => None,
            (ChannelLayout::Mono, _) => Some(vec![1.0; to]),
            (ChannelLayout::Stereo, 1) => Some(vec![0.5, 0.5]),
            (ChannelLayout::Surround5_1, 2) => Some(normalised(&DOWNMIX_5_1)),
            (ChannelLayout::Surround7_1, 2) => Some(normalised(&DOWNMIX_7_1)),
            // No defined mapping: keep the leading channels and leave the rest silent.
            _ => Some(
                (0..to * from)
                    .map(|n| if n / from == n % from { 1.0 } else { 0.0 })
                    .collect(),
            ),
        };
        Mixer { from, to, matrix }
    }

    pub fn source_channels(&self) -> usize {
        self.from
    }

    pub fn device_channels(&self) -> usize {
        self.to
    }

    /// Appends the mixed form of `input` (whole source frames) to `out`.
    pub fn mix(&self, input: &[f32], out: &mut Vec<f32>) {
        let Some(matrix) = &self.matrix else {
            out.extend_from_slice(input);
            return;
        };
        for frame in input.chunks_exact(self.from) {
            for weights in matrix.chunks_exact(self.from) {
                out.push(
                    weights
                        .iter()
                        .zip(frame)
                        .map(|(weight, sample)| weight * sample)
                        .sum(),
                );
            }
        }
    }
}

/// Scales a downmix so that every channel at full scale sums to exactly full scale: a loud
/// surround mix then cannot clip in stereo, at the cost of the BS.775 sum's 7.7 dB headroom.
fn normalised<const N: usize>(rows: &[[f32; N]; 2]) -> Vec<f32> {
    let loudest: f32 = rows
        .iter()
        .map(|row| row.iter().sum::<f32>())
        .fold(0.0, f32::max);
    rows.iter()
        .flatten()
        .map(|weight| weight / loudest)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix(layout: ChannelLayout, to: u16, frame: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        Mixer::new(layout, to).mix(frame, &mut out);
        out
    }

    fn unit(channel: usize, of: usize) -> Vec<f32> {
        (0..of)
            .map(|n| if n == channel { 1.0 } else { 0.0 })
            .collect()
    }

    const GAIN_5_1: f32 = 1.0 / (1.0 + 2.0 * FRAC_1_SQRT_2);

    #[test]
    fn layouts_are_recognised_from_speaker_masks() {
        let five_one_rear = FRONT | REAR;
        let five_one_side = FRONT | SIDE;
        assert_eq!(ChannelLayout::from_mask(5, 1), ChannelLayout::Mono);
        assert_eq!(ChannelLayout::from_mask(3, 2), ChannelLayout::Stereo);
        assert_eq!(
            ChannelLayout::from_mask(five_one_rear, 6),
            ChannelLayout::Surround5_1
        );
        assert_eq!(
            ChannelLayout::from_mask(five_one_side, 6),
            ChannelLayout::Surround5_1
        );
        assert_eq!(
            ChannelLayout::from_mask(FRONT | REAR | SIDE, 8),
            ChannelLayout::Surround7_1
        );
    }

    #[test]
    fn unlabelled_or_unusual_channel_sets_are_not_mistaken_for_surround() {
        assert_eq!(ChannelLayout::from_mask(0, 6), ChannelLayout::Other(6));
        assert_eq!(ChannelLayout::from_mask(FRONT, 6), ChannelLayout::Other(6));
        assert_eq!(
            ChannelLayout::from_mask(FRONT | REAR, 8),
            ChannelLayout::Other(8)
        );
        assert_eq!(ChannelLayout::from_mask(0b111, 3), ChannelLayout::Other(3));
        assert_eq!(ChannelLayout::Other(3).channels(), 3);
    }

    #[test]
    fn matching_channel_counts_pass_through_bit_exactly() {
        let frames = [0.1, -0.2, 0.3, 0.4, 0.5, 0.6];
        assert_eq!(mix(ChannelLayout::Stereo, 2, &frames), frames);
        assert_eq!(mix(ChannelLayout::Surround5_1, 6, &frames), frames);
    }

    #[test]
    fn mono_is_duplicated_to_every_output_channel() {
        assert_eq!(
            mix(ChannelLayout::Mono, 2, &[0.25, -0.5]),
            [0.25, 0.25, -0.5, -0.5]
        );
        assert_eq!(mix(ChannelLayout::Mono, 3, &[1.0]), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn stereo_to_mono_averages() {
        assert_eq!(mix(ChannelLayout::Stereo, 1, &[1.0, 0.5]), [0.75]);
    }

    #[test]
    fn five_one_front_channels_map_to_their_own_side() {
        assert_eq!(
            mix(ChannelLayout::Surround5_1, 2, &unit(0, 6)),
            [GAIN_5_1, 0.0]
        );
        assert_eq!(
            mix(ChannelLayout::Surround5_1, 2, &unit(1, 6)),
            [0.0, GAIN_5_1]
        );
    }

    #[test]
    fn five_one_centre_feeds_both_sides_at_minus_3_db() {
        let out = mix(ChannelLayout::Surround5_1, 2, &unit(2, 6));
        let expected = FRAC_1_SQRT_2 * GAIN_5_1;
        assert!(
            out.iter().all(|&sample| (sample - expected).abs() < 1e-7),
            "{out:?}"
        );
    }

    #[test]
    fn five_one_surrounds_feed_their_own_side_at_minus_3_db() {
        let expected = FRAC_1_SQRT_2 * GAIN_5_1;
        let left = mix(ChannelLayout::Surround5_1, 2, &unit(4, 6));
        let right = mix(ChannelLayout::Surround5_1, 2, &unit(5, 6));
        assert!(
            (left[0] - expected).abs() < 1e-7 && left[1] == 0.0,
            "{left:?}"
        );
        assert!(
            right[0] == 0.0 && (right[1] - expected).abs() < 1e-7,
            "{right:?}"
        );
    }

    #[test]
    fn lfe_is_left_out_of_the_downmix() {
        assert_eq!(mix(ChannelLayout::Surround5_1, 2, &unit(3, 6)), [0.0, 0.0]);
        assert_eq!(mix(ChannelLayout::Surround7_1, 2, &unit(3, 8)), [0.0, 0.0]);
    }

    #[test]
    fn full_scale_on_every_channel_cannot_clip() {
        for (layout, channels) in [
            (ChannelLayout::Surround5_1, 6),
            (ChannelLayout::Surround7_1, 8),
        ] {
            let out = mix(layout, 2, &vec![1.0; channels]);
            assert!(
                out.iter().all(|&sample| (sample - 1.0).abs() < 1e-6),
                "{layout:?}: {out:?}"
            );
        }
    }

    #[test]
    fn seven_one_surround_pairs_share_the_surround_weight() {
        let rear = mix(ChannelLayout::Surround7_1, 2, &unit(4, 8));
        let side = mix(ChannelLayout::Surround7_1, 2, &unit(6, 8));
        assert_eq!(rear, side);
        // The two pairs together carry one BS.775 surround: their power sums to its.
        let gain = 1.0 / (1.0 + FRAC_1_SQRT_2 + 1.0);
        assert!((rear[0] - 0.5 * gain).abs() < 1e-7, "{rear:?}");
        assert_eq!(rear[1], 0.0);
        let front = mix(ChannelLayout::Surround7_1, 2, &unit(0, 8));
        assert!((front[0] - gain).abs() < 1e-7);
    }

    #[test]
    fn unmapped_conversions_keep_leading_channels_and_silence_the_rest() {
        assert_eq!(
            mix(ChannelLayout::Other(3), 2, &[1.0, 2.0, 3.0]),
            [1.0, 2.0]
        );
        assert_eq!(mix(ChannelLayout::Stereo, 3, &[1.0, 2.0]), [1.0, 2.0, 0.0]);
        assert_eq!(
            mix(ChannelLayout::Surround5_1, 8, &unit(2, 6)),
            [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn mixing_handles_many_frames() {
        let mut input = Vec::new();
        for _ in 0..3 {
            input.extend(unit(0, 6));
        }
        let out = mix(ChannelLayout::Surround5_1, 2, &input);
        assert_eq!(out, [GAIN_5_1, 0.0, GAIN_5_1, 0.0, GAIN_5_1, 0.0]);
    }

    fn downmixed_first_frame(file: &str) -> Vec<f32> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(file);
        let mut decoder = crate::Decoder::open(&path).unwrap();
        let channels = usize::from(decoder.channels());
        let layout = decoder.channel_layout();
        let chunk = decoder.next_chunk().unwrap().expect("some audio");
        mix(layout, 2, &chunk[100 * channels..101 * channels])
    }

    #[test]
    fn a_decoded_five_one_file_downmixes_by_the_bs775_equations() {
        // Channels are 0.05 0.10 0.15 0.20 0.25 0.30: L R C LFE Ls Rs.
        let out = downmixed_first_frame("surround-5.1.flac");
        let c = FRAC_1_SQRT_2;
        let left = (0.05 + c * 0.15 + c * 0.25) * GAIN_5_1;
        let right = (0.10 + c * 0.15 + c * 0.30) * GAIN_5_1;
        assert!(
            (out[0] - left).abs() < 1e-3 && (out[1] - right).abs() < 1e-3,
            "{out:?}"
        );
    }

    #[test]
    fn a_decoded_seven_one_file_downmixes_with_shared_surround_weight() {
        // Channels are 0.05 .. 0.40: L R C LFE Lb Rb Ls Rs.
        let out = downmixed_first_frame("surround-7.1.flac");
        let c = FRAC_1_SQRT_2;
        let gain = 1.0 / (1.0 + c + 1.0);
        let left = (0.05 + c * 0.15 + 0.5 * 0.25 + 0.5 * 0.35) * gain;
        let right = (0.10 + c * 0.15 + 0.5 * 0.30 + 0.5 * 0.40) * gain;
        assert!(
            (out[0] - left).abs() < 1e-3 && (out[1] - right).abs() < 1e-3,
            "{out:?}"
        );
    }
}
