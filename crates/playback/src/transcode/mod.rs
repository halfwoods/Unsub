//! Converting audio files to other formats: for syncing to iPods and other devices, and for
//! streaming at lower bitrates.
//!
//! Decoding is the same as for playback, so anything the player reads can be converted. Encoding
//! uses FFmpeg's encoders, so the `ffmpeg` feature is required; without it conversions fail with
//! [`Error::Unsupported`].

mod ipod;

// Everything below needs FFmpeg's encoders, and is dead code without them.
#[cfg(feature = "ffmpeg")]
mod dither;
#[cfg(feature = "ffmpeg")]
mod encoder;
#[cfg(feature = "ffmpeg")]
mod job;
#[cfg(feature = "ffmpeg")]
mod plan;
#[cfg(feature = "ffmpeg")]
mod tags;

use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

pub use ipod::ipod_compatible;

use crate::Result;

/// The format to convert to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Target {
    /// Apple Lossless in an m4a.
    Alac,
    /// AAC-LC in an m4a, or ADTS when streamed.
    Aac {
        kbps: u32,
    },
    Mp3 {
        kbps: u32,
    },
    /// Opus in an Ogg container.
    Opus {
        kbps: u32,
    },
    Flac,
}

impl Target {
    /// Whether the FFmpeg build this crate is linked against has an encoder for it.
    pub fn is_available(self) -> bool {
        #[cfg(feature = "ffmpeg")]
        {
            encoder::is_available(self)
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            false
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscodeOptions {
    pub target: Target,
    /// Rates above this are brought down, keeping to the source's 44.1 or 48 kHz family where
    /// possible. Lossy targets also stay within what their format allows.
    pub max_sample_rate: Option<u32>,
    /// Lossless output is limited to this many bits per sample, with dither where it is lowered.
    pub max_bit_depth: Option<u8>,
    /// Copy tags and cover art. File conversions only.
    pub keep_tags: bool,
}

/// Converts `input` to `output` in the format `options` ask for.
///
/// The result is written to a temporary file next to `output` and renamed into place when
/// complete, so `output` is never left half-written. Output is mono or stereo; surround sources
/// are downmixed. `progress` is called with the milliseconds converted so far and the total
/// (zero when unknown). Setting `cancel` stops the conversion with [`Error::Cancelled`], leaving
/// no file behind.
///
/// [`Error::Cancelled`]: crate::Error::Cancelled
pub fn transcode_file(
    input: &Path,
    output: &Path,
    options: &TranscodeOptions,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<()> {
    #[cfg(feature = "ffmpeg")]
    {
        job::to_file(input, output, options, progress, cancel)
    }
    #[cfg(not(feature = "ffmpeg"))]
    {
        let _ = (input, output, options, progress, cancel);
        Err(unsupported())
    }
}

/// Converts `input` as the returned reader is read: decoding and encoding proceed only as fast
/// as the caller consumes the bytes. The string is the HTTP content type.
///
/// Formats that cannot be written without seeking (ALAC in m4a) are refused with
/// [`Error::Unsupported`]. Tags are not carried across, and there is no gapless marker.
///
/// [`Error::Unsupported`]: crate::Error::Unsupported
pub fn transcode_stream(
    input: &Path,
    options: &TranscodeOptions,
) -> Result<(Box<dyn Read + Send>, &'static str)> {
    #[cfg(feature = "ffmpeg")]
    {
        job::to_stream(input, options)
    }
    #[cfg(not(feature = "ffmpeg"))]
    {
        let _ = (input, options);
        Err(unsupported())
    }
}

#[cfg(not(feature = "ffmpeg"))]
fn unsupported() -> crate::Error {
    crate::Error::Unsupported("converting audio needs the `ffmpeg` feature")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_travel_as_camel_case_json() {
        let options = TranscodeOptions {
            target: Target::Aac { kbps: 128 },
            max_sample_rate: Some(48_000),
            max_bit_depth: Some(16),
            keep_tags: true,
        };
        let json = serde_json::to_value(options).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "target": { "type": "aac", "kbps": 128 },
                "maxSampleRate": 48000,
                "maxBitDepth": 16,
                "keepTags": true
            })
        );
        assert_eq!(
            serde_json::from_value::<TranscodeOptions>(json).unwrap(),
            options
        );
    }

    #[test]
    fn targets_without_a_bitrate_are_bare_tags() {
        assert_eq!(
            serde_json::to_value(Target::Alac).unwrap(),
            serde_json::json!({ "type": "alac" })
        );
        let flac: Target = serde_json::from_str(r#"{"type":"flac"}"#).unwrap();
        assert_eq!(flac, Target::Flac);
    }
}
