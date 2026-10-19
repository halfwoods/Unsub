//! Local-file audio engine: decodes with symphonia and plays through cpal.

mod decoder;
mod device_state;
mod engine;
mod error;
mod gain;
mod mp4;
mod output;
mod player;
mod resample;
mod timeline;

pub use decoder::Decoder;
pub use error::{Error, Result};
pub use player::{PlaybackState, PlaybackStatus, Player, PlayerEvent};
