//! What an iPod classic plays without conversion.

use std::path::Path;

use crate::decoder::Codec;
use crate::{Decoder, Result};

/// Whether the file plays natively on an iPod classic, so sync can copy it as it is: MP3, AAC in
/// an m4a, ALAC up to 48 kHz and 16 bits, and 16-bit WAV or AIFF, all mono or stereo and at most
/// 48 kHz.
pub fn ipod_compatible(path: &Path) -> Result<bool> {
    let decoder = Decoder::open(path)?;
    Ok(plays_natively(
        decoder.codec(),
        decoder.container(),
        decoder.sample_rate(),
        decoder.channels(),
        decoder.bit_depth(),
    ))
}

fn plays_natively(
    codec: Codec,
    container: &str,
    sample_rate: u32,
    channels: u16,
    bit_depth: Option<u32>,
) -> bool {
    if sample_rate > 48_000 || channels > 2 {
        return false;
    }
    match codec {
        Codec::Mp3 => true,
        // Raw ADTS streams (.aac) are not something iTunes or an iPod library takes.
        Codec::Aac => container == "isomp4",
        Codec::Alac => bit_depth.is_some_and(|bits| bits <= 16),
        Codec::Pcm => matches!(container, "wave" | "aiff") && bit_depth == Some(16),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mp3_and_aac_in_m4a_play() {
        assert!(plays_natively(Codec::Mp3, "mp3", 44_100, 2, None));
        assert!(plays_natively(Codec::Aac, "isomp4", 44_100, 2, None));
        assert!(!plays_natively(Codec::Aac, "adts", 44_100, 2, None));
    }

    #[test]
    fn alac_needs_16_bits_and_48_khz_or_less() {
        assert!(plays_natively(Codec::Alac, "isomp4", 48_000, 2, Some(16)));
        assert!(!plays_natively(Codec::Alac, "isomp4", 48_000, 2, Some(24)));
        assert!(!plays_natively(Codec::Alac, "isomp4", 96_000, 2, Some(16)));
        assert!(
            !plays_natively(Codec::Alac, "isomp4", 44_100, 2, None),
            "unknown depth is not assumed"
        );
    }

    #[test]
    fn wav_and_aiff_need_16_bit_pcm() {
        assert!(plays_natively(Codec::Pcm, "wave", 44_100, 1, Some(16)));
        assert!(plays_natively(Codec::Pcm, "aiff", 22_050, 2, Some(16)));
        assert!(!plays_natively(Codec::Pcm, "wave", 44_100, 2, Some(24)));
        assert!(!plays_natively(Codec::Pcm, "wave", 44_100, 2, Some(32)));
    }

    #[test]
    fn everything_else_needs_converting() {
        for codec in [
            Codec::Flac,
            Codec::Vorbis,
            Codec::Opus,
            Codec::WavPack,
            Codec::Ape,
            Codec::Dsd,
        ] {
            assert!(
                !plays_natively(codec, "flac", 44_100, 2, Some(16)),
                "{codec:?}"
            );
        }
    }

    #[test]
    fn surround_and_hi_res_rates_never_play() {
        assert!(!plays_natively(Codec::Mp3, "mp3", 44_100, 6, None));
        assert!(!plays_natively(Codec::Aac, "isomp4", 96_000, 2, None));
    }
}
