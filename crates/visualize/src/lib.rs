//! The visualizer's ears: turns audio into the spectrum, waveform, loudness, beat and tempo the
//! renderer draws, and keeps flashing within photosensitivity limits. No drawing happens here.
//!
//! - [`Analyzer`] takes audio blocks as they are mixed, with the time each will be heard, and
//!   answers [`Analyzer::frame_at`] with the [`Frame`] for the moment playing.
//! - [`MicListener`] feeds an analyzer from a microphone instead, for "React to sound".
//! - [`FlashLimiter`] caps how often a brightness may swing, per WCAG 2.3.1.
//!
//! Analysis is cheap in an optimized build (about a thousand times faster than real time) but
//! its FFT is generic code compiled into this crate, so an unoptimized build needs this crate
//! optimized to keep up: the workspace's `[profile.dev.package.unsub-visualize]` does that.
//!
//! Beat detection needs the 2048-sample window to be short against the beat, so it works from
//! about 32 kHz upwards; below that spectrum, waveform and loudness are still right but beats
//! are often missed.

mod analyzer;
mod bands;
mod error;
mod flash;
mod frame;
mod mic;
mod rhythm;
mod ring;
mod scope;
mod signal;
mod spectrum;

pub use analyzer::Analyzer;
pub use error::{Error, Result};
pub use flash::{FLASH_THRESHOLD, FlashLimiter, MAX_FLASHES_PER_SECOND, limit_flashes};
pub use frame::{BANDS, Frame, SCOPE_POINTS};
pub use mic::{MicDevice, MicListener};
