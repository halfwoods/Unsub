//! Audio encoders. Each reads a track's PCM from a `TrackAudio` and writes a finished file.

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod apple;
mod flac;

use std::path::Path;

#[cfg(not(target_os = "macos"))]
use crate::Error;
use crate::{Result, disc::TrackAudio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Free Lossless Audio Codec. `compression` is 0 (fastest) to 8 (smallest), as in the `flac`
    /// tool; higher levels spend more time searching for better predictors.
    Flac { compression: u8 },
    /// Apple Lossless in an M4A file. macOS only.
    Alac,
    /// AAC-LC in an M4A file at a target of `kbps` (32 to 320). macOS only.
    Aac { kbps: u32 },
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Flac { .. } => "flac",
            Self::Alac | Self::Aac { .. } => "m4a",
        }
    }
}

pub(crate) fn encode(format: Format, audio: &mut dyn TrackAudio, out: &Path) -> Result<()> {
    match format {
        Format::Flac { compression } => flac::encode(compression, audio, out),
        #[cfg(target_os = "macos")]
        Format::Alac | Format::Aac { .. } => apple::encode(format, audio, out),
        #[cfg(not(target_os = "macos"))]
        Format::Alac | Format::Aac { .. } => Err(Error::Unsupported("ALAC and AAC encoding")),
    }
}
