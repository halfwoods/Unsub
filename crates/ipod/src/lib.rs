//! Read-only access to classic iPods that mount as USB disks: discovery, model identification and
//! the music database (`iTunesDB`, or zlib-compressed `iTunesCDB`). The database format is
//! reverse-engineered; offsets follow libgpod (LGPL-2.1+), cited where each table is read.

mod bytes;
mod database;
mod device;
mod discover;
mod eject;
mod error;
mod itunesdb;
mod models;
mod play_counts;
mod sysinfo;
mod time;

pub use database::{Database, IpodMediaKind, IpodPlaylist, IpodTrack, Usage, read_database};
pub use device::{DatabaseFormat, Ipod, open};
pub use discover::discover;
pub use eject::eject;
pub use error::{Error, Result};
pub use models::{IpodFamily, IpodModel, SyncSupport};
