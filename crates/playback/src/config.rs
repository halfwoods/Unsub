//! Choosing a device stream format: pure functions over what a device offers.

use cpal::SampleFormat;

use crate::output::Format;

/// One stream format a device offers: a channel count, a range of rates, a sample type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub channels: u16,
    pub min_rate: u32,
    pub max_rate: u32,
    pub format: SampleFormat,
}

impl Offer {
    fn carries(&self, source: Format) -> bool {
        self.channels == source.channels
            && (self.min_rate..=self.max_rate).contains(&source.sample_rate)
    }
}

/// Sample types the output can write.
pub fn is_writable(format: SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::F32 | SampleFormat::I32 | SampleFormat::I24 | SampleFormat::I16
    )
}

/// The best sample type among the device's offers that run at the source's own rate and channel
/// count, or `None` when the device cannot, and the caller must convert to its default.
pub fn pick_native(offers: &[Offer], source: Format) -> Option<SampleFormat> {
    let rank = |format| match format {
        SampleFormat::F32 => 4,
        SampleFormat::I32 => 3,
        SampleFormat::I24 => 2,
        _ => 1,
    };
    offers
        .iter()
        .filter(|offer| offer.carries(source) && is_writable(offer.format))
        .map(|offer| offer.format)
        .max_by_key(|&format| rank(format))
}

/// The sample type for bit-perfect output, or `None` when the device cannot carry the source
/// unchanged. Integer types are preferred, the narrowest that holds every bit of the source;
/// float is exact up to 24 bits and is the choice when the device has no integer type wide
/// enough. Sources with no integer depth (lossy codecs decode to float) take the widest type.
pub fn pick_exact(
    offers: &[Offer],
    source: Format,
    bit_depth: Option<u32>,
) -> Option<SampleFormat> {
    let score = |format: SampleFormat| -> Option<i32> {
        let bits = integer_bits(format);
        match (bit_depth, bits) {
            (Some(depth), Some(bits)) if bits >= depth => Some(100 - (bits - depth) as i32),
            (Some(depth), None) if depth <= 24 => Some(50),
            (Some(_), _) => None,
            (None, Some(bits)) => Some(bits as i32),
            (None, None) => Some(100),
        }
    };
    offers
        .iter()
        .filter(|offer| offer.carries(source) && is_writable(offer.format))
        .filter_map(|offer| Some((score(offer.format)?, offer.format)))
        .max_by_key(|&(score, _)| score)
        .map(|(_, format)| format)
}

fn integer_bits(format: SampleFormat) -> Option<u32> {
    match format {
        SampleFormat::I16 => Some(16),
        SampleFormat::I24 => Some(24),
        SampleFormat::I32 => Some(32),
        _ => None,
    }
}

/// A sample layout for an exclusive-mode stream, which the device either accepts exactly or not.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Bits each sample occupies.
    pub container_bits: u16,
    /// Bits that carry signal, left-justified in the container.
    pub valid_bits: u16,
    pub float: bool,
}

#[cfg(any(windows, test))]
impl Sample {
    const fn int(container_bits: u16, valid_bits: u16) -> Sample {
        Sample {
            container_bits,
            valid_bits,
            float: false,
        }
    }

    const FLOAT: Sample = Sample {
        container_bits: 32,
        valid_bits: 32,
        float: true,
    };
}

/// Sample layouts to offer an exclusive-mode device, best first, for a source of `bit_depth`:
/// the narrowest integer layout holding every bit, then wider ones, then float where float is
/// exact. Exclusive drivers accept integers far more often than float, hence the order.
#[cfg(any(windows, test))]
pub fn exclusive_candidates(bit_depth: Option<u32>) -> Vec<Sample> {
    match bit_depth {
        Some(depth) if depth <= 16 => vec![
            Sample::int(16, 16),
            Sample::int(24, 24),
            Sample::int(32, 24),
            Sample::int(32, 32),
            Sample::FLOAT,
        ],
        Some(depth) if depth <= 24 => vec![
            Sample::int(24, 24),
            Sample::int(32, 24),
            Sample::int(32, 32),
            Sample::FLOAT,
        ],
        // Float cannot hold 32 integer bits.
        Some(_) => vec![Sample::int(32, 32)],
        None => vec![
            Sample::int(32, 32),
            Sample::int(32, 24),
            Sample::int(24, 24),
            Sample::FLOAT,
            Sample::int(16, 16),
        ],
    }
}

/// Packs left-justified 32-bit samples into 3-byte little-endian ones, dropping the low byte.
#[cfg(any(windows, test))]
pub fn pack_24_bit(samples: &[i32], out: &mut [u8]) {
    for (sample, bytes) in samples.iter().zip(out.chunks_exact_mut(3)) {
        bytes.copy_from_slice(&sample.to_le_bytes()[1..]);
    }
}

/// Bits per sample and numeric type, as shown to the user: "24-bit", "32-bit float".
pub fn depth_label(format: SampleFormat) -> &'static str {
    match format {
        SampleFormat::I16 => "16-bit",
        SampleFormat::I24 => "24-bit",
        SampleFormat::I32 => "32-bit",
        _ => "32-bit float",
    }
}

/// "24-bit 96 kHz".
pub fn format_label(depth: &str, sample_rate: u32) -> String {
    let khz = format!("{:.3}", f64::from(sample_rate) / 1000.0);
    let khz = khz.trim_end_matches('0').trim_end_matches('.');
    format!("{depth} {khz} kHz")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(format: SampleFormat, rates: (u32, u32)) -> Offer {
        Offer {
            channels: 2,
            min_rate: rates.0,
            max_rate: rates.1,
            format,
        }
    }

    const STEREO_96K: Format = Format {
        sample_rate: 96_000,
        channels: 2,
    };

    #[test]
    fn native_prefers_float_then_wider_integers() {
        let offers = [
            offer(SampleFormat::I16, (44_100, 96_000)),
            offer(SampleFormat::I32, (44_100, 96_000)),
            offer(SampleFormat::I24, (44_100, 96_000)),
        ];
        assert_eq!(pick_native(&offers, STEREO_96K), Some(SampleFormat::I32));
        let with_float = [offers[0], offer(SampleFormat::F32, (96_000, 96_000))];
        assert_eq!(
            pick_native(&with_float, STEREO_96K),
            Some(SampleFormat::F32)
        );
    }

    #[test]
    fn native_needs_the_exact_rate_and_channels() {
        let offers = [offer(SampleFormat::F32, (44_100, 48_000))];
        assert_eq!(pick_native(&offers, STEREO_96K), None);
        let mono = Format {
            sample_rate: 44_100,
            channels: 1,
        };
        assert_eq!(pick_native(&offers, mono), None);
    }

    #[test]
    fn unwritable_sample_types_are_never_chosen() {
        let offers = [
            offer(SampleFormat::U8, (96_000, 96_000)),
            offer(SampleFormat::F64, (96_000, 96_000)),
        ];
        assert_eq!(pick_native(&offers, STEREO_96K), None);
        assert_eq!(pick_exact(&offers, STEREO_96K, Some(16)), None);
    }

    #[test]
    fn exact_takes_the_integer_type_matching_the_source_depth() {
        let all = [
            offer(SampleFormat::I16, (44_100, 192_000)),
            offer(SampleFormat::I24, (44_100, 192_000)),
            offer(SampleFormat::I32, (44_100, 192_000)),
            offer(SampleFormat::F32, (44_100, 192_000)),
        ];
        assert_eq!(
            pick_exact(&all, STEREO_96K, Some(16)),
            Some(SampleFormat::I16)
        );
        assert_eq!(
            pick_exact(&all, STEREO_96K, Some(24)),
            Some(SampleFormat::I24)
        );
        assert_eq!(
            pick_exact(&all, STEREO_96K, Some(32)),
            Some(SampleFormat::I32)
        );
    }

    #[test]
    fn exact_widens_to_the_next_integer_type_rather_than_narrowing() {
        let no_16 = [
            offer(SampleFormat::I24, (96_000, 96_000)),
            offer(SampleFormat::I32, (96_000, 96_000)),
        ];
        assert_eq!(
            pick_exact(&no_16, STEREO_96K, Some(16)),
            Some(SampleFormat::I24)
        );
        let only_24 = [offer(SampleFormat::I24, (96_000, 96_000))];
        assert_eq!(
            pick_exact(&only_24, STEREO_96K, Some(32)),
            None,
            "24 bits cannot hold 32"
        );
    }

    #[test]
    fn exact_uses_float_when_no_integer_type_is_wide_enough() {
        let float_only = [offer(SampleFormat::F32, (44_100, 96_000))];
        assert_eq!(
            pick_exact(&float_only, STEREO_96K, Some(16)),
            Some(SampleFormat::F32)
        );
        assert_eq!(
            pick_exact(&float_only, STEREO_96K, Some(24)),
            Some(SampleFormat::F32)
        );
        assert_eq!(
            pick_exact(&float_only, STEREO_96K, Some(32)),
            None,
            "f32 cannot hold 32-bit integers"
        );
    }

    #[test]
    fn exact_prefers_an_integer_type_over_float_for_integer_sources() {
        let offers = [
            offer(SampleFormat::F32, (96_000, 96_000)),
            offer(SampleFormat::I24, (96_000, 96_000)),
        ];
        assert_eq!(
            pick_exact(&offers, STEREO_96K, Some(24)),
            Some(SampleFormat::I24)
        );
    }

    #[test]
    fn exact_gives_lossy_sources_the_widest_type() {
        let offers = [
            offer(SampleFormat::I16, (96_000, 96_000)),
            offer(SampleFormat::I32, (96_000, 96_000)),
            offer(SampleFormat::F32, (96_000, 96_000)),
        ];
        assert_eq!(
            pick_exact(&offers, STEREO_96K, None),
            Some(SampleFormat::F32)
        );
        let ints = &offers[..2];
        assert_eq!(pick_exact(ints, STEREO_96K, None), Some(SampleFormat::I32));
    }

    #[test]
    fn exact_refuses_a_device_that_cannot_match_rate_or_channels() {
        let offers = [offer(SampleFormat::F32, (44_100, 48_000))];
        assert_eq!(pick_exact(&offers, STEREO_96K, Some(16)), None);
    }

    #[test]
    fn labels_read_naturally() {
        assert_eq!(format_label("24-bit", 96_000), "24-bit 96 kHz");
        assert_eq!(format_label("16-bit", 44_100), "16-bit 44.1 kHz");
        assert_eq!(
            format_label("32-bit float", 352_800),
            "32-bit float 352.8 kHz"
        );
        assert_eq!(format_label("16-bit", 22_050), "16-bit 22.05 kHz");
        assert_eq!(format_label("24-bit", 176_400), "24-bit 176.4 kHz");
        assert_eq!(depth_label(SampleFormat::I24), "24-bit");
        assert_eq!(depth_label(SampleFormat::F32), "32-bit float");
    }

    #[test]
    fn exclusive_candidates_start_with_the_narrowest_integer_that_fits() {
        assert_eq!(exclusive_candidates(Some(16))[0], Sample::int(16, 16));
        assert_eq!(exclusive_candidates(Some(24))[0], Sample::int(24, 24));
        assert_eq!(exclusive_candidates(Some(32)), [Sample::int(32, 32)]);
        assert_eq!(exclusive_candidates(None)[0], Sample::int(32, 32));
    }

    #[test]
    fn exclusive_candidates_never_narrow_the_source() {
        for depth in [8, 16, 20, 24] {
            for candidate in exclusive_candidates(Some(depth)) {
                let holds = candidate.float || u32::from(candidate.valid_bits) >= depth;
                assert!(holds, "{candidate:?} cannot carry {depth} bits");
            }
        }
        assert!(exclusive_candidates(Some(32)).iter().all(|c| !c.float));
    }

    #[test]
    fn packing_keeps_the_top_three_bytes_in_little_endian_order() {
        let mut out = [0u8; 6];
        pack_24_bit(
            &[0x1234_5600, i32::from_be_bytes([0xFF, 0x80, 0x00, 0x00])],
            &mut out,
        );
        assert_eq!(out, [0x56, 0x34, 0x12, 0x00, 0x80, 0xFF]);
    }
}
