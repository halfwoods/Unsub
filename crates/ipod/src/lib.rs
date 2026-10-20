//! Classic iPods that mount as USB disks: discovery, model identification, the music database
//! (`iTunesDB`, or zlib-compressed `iTunesCDB`), and syncing music to the ones whose database can
//! be written safely. The database format is reverse-engineered; offsets follow libgpod
//! (LGPL-2.1+), cited where each table is read or written.

mod bytes;
mod database;
mod device;
mod discover;
mod eject;
mod error;
mod itunesdb;
mod models;
mod play_counts;
mod sync;
mod sysinfo;
mod time;
mod verify;

pub use database::{Database, IpodMediaKind, IpodPlaylist, IpodTrack, Usage, read_database};
pub use device::{DatabaseFormat, Ipod, open};
pub use discover::discover;
pub use eject::eject;
pub use error::{Error, Result};
pub use models::{IpodFamily, IpodModel, SyncSupport};
pub use sync::{
    OnTheGo, PlayDelta, SyncPlan, SyncPlaylist, SyncProgress, SyncStage, SyncTrack, apply_sync,
    artwork_supported, clear_on_the_go, clear_play_counts, plan_sync, read_on_the_go,
    read_play_counts_for_sync_back, restore_backup,
};
pub use verify::{
    ArtworkCheck, ChecksumCheck, ChunkCount, FieldDifference, HeaderChange, RecordDifference,
    RoundtripReport, SectionSize, verify_roundtrip,
};
