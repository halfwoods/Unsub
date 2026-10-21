//! Local-file audio engine: decodes with symphonia (and FFmpeg for Opus, WavPack, APE and DSD)
//! and plays through cpal, or through exclusive-mode WASAPI/hog-mode CoreAudio when bit-perfect.

#[cfg(test)]
mod alloc_probe;
mod config;
mod crossfade;
mod decoder;
#[cfg(target_os = "macos")]
mod device_state;
mod engine;
mod equalizer;
mod error;
#[cfg(feature = "ffmpeg")]
mod ffmpeg_source;
mod gain;
mod host;
mod live;
mod loudness;
mod mix;
mod mp4;
mod output;
mod player;
mod resample;
#[cfg(test)]
mod scenarios;
mod symphonia_source;
mod tap;
mod timeline;
mod transcode;
#[cfg(windows)]
mod wasapi_exclusive;

pub use decoder::{Backend, Codec, Decoder};
pub use equalizer::{Equalizer, PRESETS, Preset};
pub use error::{Error, Result};
pub use gain::Gain;
pub use live::LiveFormat;
pub use loudness::{BlockHistogram, Loudness, album_loudness, analyze_loudness};
pub use mix::ChannelLayout;
pub use player::{PlaybackState, PlaybackStatus, Player, PlayerEvent};
pub use tap::{Tap, TapBlock};
pub use timeline::TrackOptions;
pub use transcode::{Target, TranscodeOptions, ipod_compatible, transcode_file, transcode_stream};

/// Whether the system's output is an AirPlay speaker, chosen in Control Center or System Settings.
/// The engine follows the system's default output wherever it moves, so nothing else is needed to
/// play to one. Always false off macOS, which has no AirPlay output to follow.
pub fn output_is_airplay() -> bool {
    #[cfg(target_os = "macos")]
    {
        device_state::default_output_is_airplay()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
