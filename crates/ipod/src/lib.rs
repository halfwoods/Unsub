//! Classic iPods that mount as USB disks: discovery, model identification, the music database
//! (`iTunesDB`, or zlib-compressed `iTunesCDB`), syncing music to the ones whose database can be
//! written safely, and reading music back off any of them. The database format is reverse-engineered; offsets follow libgpod
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
mod usb;
mod verify;
pub mod video;

pub use database::{Database, IpodMediaKind, IpodPlaylist, IpodTrack, Usage, read_database};
pub use device::{DatabaseFormat, Ipod, open};
pub use discover::discover;
pub use eject::eject;
pub use error::{Error, Result};
pub use models::{IpodFamily, IpodModel, SyncSupport};
pub use sync::{
    OnTheGo, PlayDelta, Prepared, Rescue, RescuePlaylist, RescueSong, SyncPlan, SyncPlaylist,
    SyncProgress, SyncStage, SyncTrack, VideoAddPlan, VideoAddResult, VideoSyncTrack, apply_sync,
    apply_video_additions, artwork_supported, clear_on_the_go, clear_play_counts, ensure_room,
    free_space, holds_protected_audio, is_copy_protected, plan_sync, plan_video_additions,
    read_on_the_go, read_play_counts_for_sync_back, read_rescue, restore_backup,
    video_add_supported,
};
pub use verify::{
    ArtworkCheck, ChecksumCheck, ChunkCount, FieldDifference, HeaderChange, RecordDifference,
    RoundtripReport, SectionSize, verify_roundtrip,
};
