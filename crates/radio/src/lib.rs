//! Internet radio: the Radio Browser station directory, playlist files, and live streams.
//!
//! - [`Directory`] lists stations by genre and searches them.
//! - [`resolve`] turns the address of a `.pls` or `.m3u` into the streams it names.
//! - [`open`] connects to a station and returns a [`LiveStream`], a [`Read`](std::io::Read) of
//!   audio bytes alone: in-band metadata is stripped out and reported as events, dropped
//!   connections are re-established, and HLS is joined into one continuous stream. Decoding is the
//!   caller's job; [`LiveStream::format`] says what to expect.

mod directory;
mod error;
mod http;
mod live;
mod playlist;
mod stall;
#[cfg(test)]
mod testing;
mod text;

pub use directory::{Directory, GENRES, Genre, Station, Tag, genre, genres_of};
pub use error::{Error, Result};
pub use http::{CancelHandle, HttpConfig};
pub use live::{Event, Format, LiveStream, NowPlaying, Options, StationInfo, open, open_with};
pub use playlist::{Playlist, parse as parse_playlist, resolve, resolve_with};
