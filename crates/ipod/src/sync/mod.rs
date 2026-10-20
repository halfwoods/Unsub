//! Writing to the iPod: planning what a sync changes, applying it safely, and handing back what
//! the iPod recorded while it was away.
//!
//! The rules this module keeps, because the device holds music that cannot be replaced:
//! * only iPods whose database this crate can write correctly are touched (see [`checksum_key`]),
//!   and a shuffle's iTunesSD is written with its iTunesDB;
//! * only tracks and playlists this app added (recorded in `Unsub.json`) are ever removed
//!   or edited, everything else is carried over byte for byte;
//! * music is copied before the database that references it is committed, and removed files are
//!   deleted only after it;
//! * the new database replaces the old one by atomic rename, with the old one kept beside it, so a
//!   failure at any point leaves a database that still works;
//! * album art is only ever appended to the thumbnail files and the ArtworkDB, so art this app did
//!   not add is never touched, and a failed sync truncates what it appended.

mod apply;
mod artwork_support;
mod commit;
mod files;
mod mapping;
mod on_the_go;
mod plan;
mod play_deltas;
mod restore;

use std::path::PathBuf;

use serde::Serialize;

use crate::database::IpodMediaKind;
use crate::device::{DatabaseFormat, Ipod, itunes_dir};
use crate::error::{Error, Result};
use crate::itunesdb::shuffle::{self, ShuffleFormat};
use crate::models::{IpodFamily, SyncSupport};
use crate::sysinfo;

pub use apply::apply_sync;
pub use artwork_support::artwork_supported;
pub use on_the_go::{OnTheGo, clear_on_the_go, read_on_the_go};
pub use plan::plan_sync;
pub use play_deltas::{PlayDelta, clear_play_counts, read_play_counts_for_sync_back};
pub use restore::restore_backup;

/// A library track as the sync sees it. Tags describe the track on the iPod; the audio file comes
/// from `source_path`, via the `prepare` step of [`apply_sync`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncTrack {
    pub source_path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub composer: String,
    pub year: Option<i32>,
    pub track_number: Option<u32>,
    pub track_count: Option<u32>,
    pub disc_number: Option<u32>,
    pub disc_count: Option<u32>,
    /// Describes the file that ends up on the iPod (after any transcoding); not compared when
    /// deciding whether an existing track needs updating.
    pub duration_ms: u64,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    /// 0 to 100.
    pub rating: u8,
    pub play_count: u32,
    /// Unix seconds.
    pub last_played: Option<i64>,
    /// Unix seconds.
    pub date_added: i64,
    pub media_kind: IpodMediaKind,
    pub compilation: bool,
    /// iTunes' Sound Check value for the track (`1000 * 10^(-gain/10)`), or `None` for off.
    pub sound_check: Option<u32>,
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
    /// A cheap identity for this track's cover (a hash or modification time, say), or `None` for
    /// no cover. Covers are not passed in here: `apply_sync` asks for the bytes of the few it
    /// needs, one at a time. A key that differs from the one recorded when the cover was last
    /// written marks the track for update, so a changed cover is noticed without loading it.
    /// Tracks of one album with the same key are given the same cover, fetched once.
    pub artwork_key: Option<u64>,
    /// The app's id for the track, which must stay the same for the same track across syncs: it is
    /// how a later sync finds the copy on the iPod to update or remove.
    pub library_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlaylist {
    pub name: String,
    /// `library_id`s in playlist order.
    pub tracks: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlan {
    /// `library_id`s to copy to the iPod.
    pub add: Vec<i64>,
    /// `library_id`s already on the iPod whose tags differ.
    pub update: Vec<i64>,
    /// iPod track ids, all of tracks this app added earlier whose `library_id` is no longer wanted.
    pub remove: Vec<u32>,
    /// Size of the source files of `add`, plus the thumbnails of the covers that will be added. A
    /// prepared (transcoded) file can differ; the source is a safe estimate when transcoding
    /// shrinks, and is exact for plain copies.
    pub bytes_needed: u64,
    /// Whether `bytes_needed` plus working space for the database rewrite fits in the free space,
    /// counting the space `remove` frees when `removals_first` says it must be freed first.
    pub fits: bool,
    /// True when the sync fits only if the tracks in `remove` are deleted before anything is
    /// copied. Then `apply_sync` commits that removal first, as its own step: a failure later
    /// leaves the iPod with fewer tracks than before, never more.
    pub removals_first: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncStage {
    Preparing,
    Copying,
    WritingDatabase,
    Cleaning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub stage: SyncStage,
    pub done: u64,
    pub total: u64,
    pub current: String,
}

/// Decides whether this iPod's database can be written correctly and returns the key its checksum
/// needs, if any. Anything this crate cannot write exactly is refused: a wrong checksum or a missing
/// companion file makes the iPod ignore the whole library.
pub(crate) fn checksum_key(ipod: &Ipod) -> Result<Option<[u8; 8]>> {
    let refuse = |reason: &str| Err(Error::SyncRefused(reason.to_owned()));
    if ipod.database == DatabaseFormat::ITunesCdb {
        return refuse(
            "its library is a compressed iTunesCDB (SQLite) database, which cannot be written yet",
        );
    }
    // Every caller of this gate must also refuse a device whose iTunesSD cannot be kept in step.
    shuffle_format(ipod)?;
    match ipod.sync {
        SyncSupport::Supported => Ok(None),
        SyncSupport::NeedsHash58 => {
            let guid = ipod.firewire_guid.as_deref().and_then(parse_guid);
            match guid {
                Some(guid) => Ok(Some(guid)),
                None => refuse(
                    "its FireWire GUID is missing or unreadable, and the database checksum depends on it",
                ),
            }
        }
        SyncSupport::NeedsHash72 => refuse(
            "it needs a HASH72 checksum, which requires a HashInfo file from iTunes and is not supported",
        ),
        SyncSupport::Unsupported => {
            refuse("its model is unknown or uses a checksum (HASHAB) that cannot be written")
        }
    }
}

/// Which iTunesSD this iPod needs beside its iTunesDB, if any. A shuffle plays from the iTunesSD
/// alone, so one that only got a new iTunesDB would keep playing the old list and lose the files
/// that were deleted. libgpod finds no database checksum for any shuffle generation
/// (`itdb_device_get_checksum_type`) and its 3G/4G writer has no authentication step.
pub(crate) fn shuffle_format(ipod: &Ipod) -> Result<Option<ShuffleFormat>> {
    let mount = ipod.mount_path.as_path();
    let refuse = |reason: &str| Err(Error::SyncRefused(reason.to_owned()));
    let Some(model) = ipod
        .model
        .as_ref()
        .filter(|model| model.family == IpodFamily::Shuffle)
    else {
        return if itunes_dir(mount).join(shuffle::FILE_NAME).exists() {
            refuse(
                "it has an iTunesSD file but is not a recognised iPod shuffle, so its song list \
                 cannot be kept in step",
            )
        } else {
            Ok(None)
        };
    };
    let shadow = sysinfo::read(&mount.join("iPod_Control").join("Device")).shadow_db_version;
    match (shadow, model.generation) {
        (Some(1), _) | (None | Some(0), 1 | 2) => Ok(Some(ShuffleFormat::Fixed)),
        (Some(2), _) | (None | Some(0), 3 | 4) => Ok(Some(ShuffleFormat::Indexed)),
        _ => refuse("its iTunesSD format is not one this crate knows"),
    }
}

/// 16 hex digits, the form every iPod's FireWire GUID has.
pub(crate) fn parse_guid(guid: &str) -> Option<[u8; 8]> {
    if guid.len() != 16 {
        return None;
    }
    let mut bytes = [0u8; 8];
    for (byte, pair) in bytes.iter_mut().zip(guid.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(bytes)
}
