//! Deciding what a conversion produces: pure functions over the source's properties.

use super::Target;

const MP3_RATES: &[u32] = &[
    48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
];
const AAC_RATES: &[u32] = &[
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];
const OPUS_RATES: &[u32] = &[48_000, 24_000, 16_000, 12_000, 8_000];
const COMMON_RATES: &[u32] = &[
    192_000, 176_400, 96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000,
    12_000, 11_025, 8_000,
];

/// Lossless output above this is storage spent on nothing; it also brings DSD's 352.8 kHz down.
const LOSSLESS_CEILING: u32 = 192_000;

/// The sample rate the output runs at: the source's own where the format allows it and
/// `max_sample_rate` does not forbid it, otherwise the best allowed rate below.
pub fn output_rate(source: u32, target: Target, max: Option<u32>) -> u32 {
    let cap = max.unwrap_or(u32::MAX);
    match target {
        // Opus is coded at 48 kHz whatever the source was.
        Target::Opus { .. } => OPUS_RATES
            .iter()
            .copied()
            .find(|&rate| rate <= cap)
            .unwrap_or(8_000),
        Target::Mp3 { .. } => choose(MP3_RATES, source, cap),
        Target::Aac { .. } => choose(AAC_RATES, source, cap),
        Target::Alac | Target::Flac => {
            let cap = cap.min(LOSSLESS_CEILING);
            if source <= cap {
                source
            } else {
                choose(COMMON_RATES, source, cap)
            }
        }
    }
}

/// The best of `allowed` for `source` under `cap`: the largest rate no higher than either. An
/// even divisor of the source is taken in its place when it is nearly as high, since converting
/// by a whole ratio is cleaner: 88.2 kHz goes to 44.1 rather than 48, but 44.1 kHz capped at 32
/// goes to 32 rather than 22.05.
fn choose(allowed: &[u32], source: u32, cap: u32) -> u32 {
    let limit = source.min(cap);
    let mut fits = allowed.iter().copied().filter(|&rate| rate <= limit);
    let Some(highest) = fits.next() else {
        return allowed.last().copied().unwrap_or(source);
    };
    allowed
        .iter()
        .copied()
        .filter(|&rate| rate <= limit && source.is_multiple_of(rate))
        .find(|&rate| u64::from(rate) * 10 >= u64::from(highest) * 9)
        .unwrap_or(highest)
}

/// Bits per sample for lossless output: 16 or 24, the source's own up to `max`.
pub fn output_depth(source_depth: Option<u32>, source_rate: u32, max: Option<u8>) -> u32 {
    let native = match source_depth {
        Some(depth) if depth <= 16 => 16,
        Some(_) => 24,
        // Lossy sources have no depth; 16 bits holds them. DSD is lossless and rich in headroom.
        None if source_rate > 48_000 => 24,
        None => 16,
    };
    let ceiling = match max {
        Some(bits) if bits <= 16 => 16,
        _ => 24,
    };
    native.min(ceiling)
}

/// Whether rounding to `depth` bits needs dither. Samples that pass through unchanged were
/// integers already and must stay exactly so.
pub fn needs_dither(depth: u32, unchanged: bool) -> bool {
    depth <= 16 && !unchanged
}

/// Transcoded audio is mono or stereo: iPods, cellular streams and car stereos want no more.
pub fn output_channels(source: u16) -> u16 {
    source.min(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AAC: Target = Target::Aac { kbps: 128 };
    const MP3: Target = Target::Mp3 { kbps: 128 };
    const OPUS: Target = Target::Opus { kbps: 96 };

    #[test]
    fn rates_the_format_allows_are_kept() {
        assert_eq!(output_rate(44_100, AAC, None), 44_100);
        assert_eq!(output_rate(48_000, MP3, None), 48_000);
        assert_eq!(output_rate(22_050, MP3, None), 22_050);
        assert_eq!(output_rate(96_000, Target::Alac, None), 96_000);
        assert_eq!(output_rate(44_100, Target::Flac, None), 44_100);
    }

    #[test]
    fn a_cap_brings_hi_res_down_within_the_same_family() {
        assert_eq!(output_rate(96_000, Target::Alac, Some(48_000)), 48_000);
        assert_eq!(output_rate(88_200, Target::Alac, Some(48_000)), 44_100);
        assert_eq!(output_rate(192_000, Target::Alac, Some(48_000)), 48_000);
        assert_eq!(output_rate(176_400, Target::Flac, Some(48_000)), 44_100);
    }

    #[test]
    fn a_cap_between_family_rates_takes_the_highest_allowed_not_a_half() {
        assert_eq!(output_rate(44_100, AAC, Some(32_000)), 32_000);
        assert_eq!(output_rate(96_000, Target::Alac, Some(44_100)), 44_100);
        assert_eq!(output_rate(48_000, MP3, Some(40_000)), 32_000);
    }

    #[test]
    fn a_cap_never_raises_a_rate() {
        assert_eq!(output_rate(44_100, Target::Alac, Some(48_000)), 44_100);
        assert_eq!(output_rate(22_050, AAC, Some(48_000)), 22_050);
    }

    #[test]
    fn mp3_cannot_exceed_48_khz_and_aac_96() {
        assert_eq!(output_rate(96_000, MP3, None), 48_000);
        assert_eq!(output_rate(88_200, MP3, None), 44_100);
        assert_eq!(output_rate(192_000, AAC, None), 96_000);
        assert_eq!(output_rate(176_400, AAC, None), 88_200);
    }

    #[test]
    fn opus_is_coded_at_48_khz_unless_capped_lower() {
        assert_eq!(output_rate(44_100, OPUS, None), 48_000);
        assert_eq!(output_rate(96_000, OPUS, None), 48_000);
        assert_eq!(output_rate(44_100, OPUS, Some(32_000)), 24_000);
        assert_eq!(output_rate(44_100, OPUS, Some(16_000)), 16_000);
    }

    #[test]
    fn lossless_output_stops_at_192_khz_and_brings_dsd_to_the_44_1_family() {
        assert_eq!(output_rate(352_800, Target::Flac, None), 176_400);
        assert_eq!(output_rate(384_000, Target::Alac, None), 192_000);
    }

    #[test]
    fn odd_source_rates_fall_to_the_largest_allowed_below() {
        assert_eq!(output_rate(37_800, MP3, None), 32_000);
        assert_eq!(output_rate(4_000, MP3, None), 8_000);
    }

    #[test]
    fn depth_follows_the_source_up_to_the_cap() {
        assert_eq!(output_depth(Some(16), 44_100, None), 16);
        assert_eq!(output_depth(Some(24), 96_000, None), 24);
        assert_eq!(output_depth(Some(32), 96_000, None), 24);
        assert_eq!(output_depth(Some(24), 96_000, Some(16)), 16);
        assert_eq!(output_depth(Some(16), 44_100, Some(24)), 16);
    }

    #[test]
    fn lossy_sources_get_16_bits_and_dsd_gets_24() {
        assert_eq!(output_depth(None, 44_100, None), 16);
        assert_eq!(output_depth(None, 352_800, None), 24);
        assert_eq!(output_depth(None, 352_800, Some(16)), 16);
    }

    #[test]
    fn dither_applies_to_16_bit_output_that_is_not_a_straight_copy() {
        assert!(needs_dither(16, false));
        assert!(!needs_dither(16, true));
        assert!(!needs_dither(24, false));
    }

    #[test]
    fn surround_comes_out_as_stereo() {
        assert_eq!(output_channels(1), 1);
        assert_eq!(output_channels(2), 2);
        assert_eq!(output_channels(6), 2);
    }
}
