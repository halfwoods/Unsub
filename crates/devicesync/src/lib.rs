//! Syncing a music library to players that are not iPods: USB mass-storage players, Rockbox
//! players and Android phones over MTP.
//!
//! ```text
//! discover()  ->  plan(device, items, playlists)  ->  apply(device, plan, items, playlists, ...)
//! ```
//!
//! The device keeps `.antistreaming/manifest.json`, the record of the files this app put there. A
//! re-sync copies only what changed and deletes only what the manifest lists, so the user's own
//! files on the player are never touched. Every write goes to a temporary name first and is
//! renamed into place, and the manifest is saved atomically after each batch, so pulling the cable
//! at any moment leaves a state the next sync understands.
//!
//! Apple-firmware iPods are the iPod crate's business and are not reported by [`discover`].

mod apply;
mod device;
mod discover;
mod error;
mod inventory;
mod layout;
mod manifest;
mod model;
mod mtp;
mod plan;
mod playlist;
#[cfg(test)]
mod scenarios;
mod storage;
#[cfg(test)]
mod testing;

pub use apply::{SyncProgress, SyncStage, apply, apply_with};
pub use device::{DEFAULT_LAYOUT, Device, DeviceKind, DeviceRoot, PlaylistFormat, Profile};
pub use discover::{MARKER_FILE, discover, mark_device};
pub use error::{Error, Result};
pub use model::{SyncItem, SyncPlaylist};
pub use mtp::{MtpBackend, MtpDevice, MtpStorageInfo};
pub use plan::{PlannedFile, RESERVE_BYTES, RemovedFile, SyncPlan, plan, plan_with};
pub use storage::{FsStorage, Storage, StoredFile, Target};
