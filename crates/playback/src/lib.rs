//! Local-file audio engine: decodes with symphonia (and FFmpeg for Opus, WavPack, APE and DSD)
//! and plays through cpal, or through exclusive-mode WASAPI/hog-mode CoreAudio when bit-perfect.

mod config;
mod decoder;
#[cfg(target_os = "macos")]
mod device_state;
mod engine;
mod error;
#[cfg(feature = "ffmpeg")]
mod ffmpeg_source;
mod gain;
mod host;
mod mix;
mod mp4;
mod output;
mod player;
mod resample;
#[cfg(test)]
mod scenarios;
mod symphonia_source;
mod timeline;
mod transcode;
#[cfg(windows)]
mod wasapi_exclusive;

pub use decoder::{Backend, Codec, Decoder};
pub use error::{Error, Result};
pub use mix::ChannelLayout;
pub use player::{PlaybackState, PlaybackStatus, Player, PlayerEvent};
pub use transcode::{Target, TranscodeOptions, ipod_compatible, transcode_file, transcode_stream};
