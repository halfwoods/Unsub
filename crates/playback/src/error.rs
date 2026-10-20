use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot read audio file: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot decode audio: {0}")]
    Decode(#[from] symphonia::core::errors::Error),
    #[error("file has no playable audio track")]
    NoAudioTrack,
    #[error("ffmpeg: {0}")]
    Ffmpeg(String),
    #[error("the decoder crashed on this file: {0}")]
    Panicked(String),
    #[error("the conversion was cancelled")]
    Cancelled,
    #[error("cannot read tags: {0}")]
    TagsRead(#[from] lofty::error::FileParseError),
    #[error("cannot write tags: {0}")]
    TagsWrite(#[from] lofty::error::FileEncodingError),
    #[error("file contains no decodable audio")]
    Empty,
    #[error("unsupported audio: {0}")]
    Unsupported(&'static str),
    #[error("audio output: {0}")]
    Output(#[from] cpal::Error),
    #[error("bit-perfect output is not possible: {0}")]
    BitPerfectUnavailable(String),
    #[error("audio device: {0}")]
    Device(String),
    #[error("no audio output device available")]
    NoDevice,
    #[error("audio output does not accept any sample format this player can write")]
    NoSampleFormat,
    #[error("audio output stopped consuming samples")]
    OutputStalled,
    #[error("cannot start sample rate conversion: {0}")]
    ResamplerSetup(#[from] rubato::ResamplerConstructionError),
    #[error("sample rate conversion failed: {0}")]
    Resample(#[from] rubato::ResampleError),
    #[error("resampler buffer mismatch: {0}")]
    ResampleBuffer(#[from] rubato::audioadapter_buffers::SizeError),
}

#[cfg(feature = "ffmpeg")]
impl From<ffmpeg_next::Error> for Error {
    fn from(error: ffmpeg_next::Error) -> Self {
        Error::Ffmpeg(error.to_string())
    }
}
