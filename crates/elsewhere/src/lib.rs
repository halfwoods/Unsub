//! What a streaming service gives you when you ask for your data, read into one shape: the
//! playlists, and the plays with when they happened. Spotify (account data, extended streaming
//! history and Exportify CSVs), YouTube Music (Google Takeout) and ListenBrainz are understood;
//! Last.fm has no file export, so its history comes in through ListenBrainz.
//!
//! Nothing here touches the library: the app matches these songs against the music a person owns.

mod csv;
mod error;
mod labels;
mod listenbrainz;
mod model;
mod read;
mod spotify;
mod time;
mod youtube;

pub use error::{Error, Result};
pub use model::{Export, Play, Playlist, Service, Song};
pub use read::read;
