//! An embeddable Subsonic / OpenSubsonic server.
//!
//! The crate knows nothing about the application's database: the host implements [`MusicSource`]
//! and hands it to [`Server::start`], which serves the Subsonic REST API (v1.16.1 plus the
//! OpenSubsonic extensions it supports) so phone apps can browse and stream the library.
//!
//! `stream` sends a song as stored, with `Range` support, unless the client's `format` or
//! `maxBitRate` (or the server's default cap, [`ServerConfig::default_max_kbps`]) call for a
//! conversion that the source can do through [`MusicSource::transcoded`]; that is sent chunked.
//! `download` is always the stored file. `scrobble` reaches the source as [`MusicSource::scrobble`]
//! or, for "now playing" pings, [`MusicSource::now_playing`].
//!
//! The server speaks plain HTTP. Legacy `u`/`p` authentication sends the password in the clear, so
//! bind it to a trusted network only.

#![forbid(unsafe_code)]

mod api;
mod auth;
mod error;
mod model;
mod params;
mod range;
mod render;
mod server;
mod source;
mod wire;

pub use error::{Error, Result};
pub use model::{Album, Artist, Genre, Playlist, SearchResult, Song};
pub use server::{Server, ServerConfig};
pub use source::{AlbumListKind, MusicSource, Stream, StreamFormat};
