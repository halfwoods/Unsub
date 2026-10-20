//! The "client mode" half of owning your own cloud: finding other AntiStreaming libraries on the
//! network, talking to any Subsonic-compatible server (another AntiStreaming, Navidrome), and
//! fetching songs into a cache so the local player can play them.

mod cache;
mod client;
mod discovery;
mod download;
mod error;
mod model;

pub use client::Client;
pub use discovery::{Advertisement, Browser, Peer, advertise, browse};
pub use download::{Download, GrowingReader, Progress};
pub use error::{Error, Result};
pub use model::{
    Album, AlbumDetail, AlbumListKind, Artist, ArtistDetail, Playlist, PlaylistDetail,
    SearchResults, Song,
};
