//! Writing to the iPod: planning what a sync changes, applying it safely, and handing back what
//! the iPod recorded while it was away.
//!
//! The rules this module keeps, because the device holds music that cannot be replaced:
//! * only iPods whose database this crate can write correctly are touched (see [`checksum_key`]);
//! * only tracks and playlists this app added (recorded in `AntiStreaming.json`) are ever removed
//!   or edited, everything else is carried over byte for byte;
//! * music is copied before the database that references it is committed, and removed files are
//!   deleted only after it;
//! * the new database replaces the old one by atomic rename, with the old one kept beside it, so a
//!   failure at any point leaves a database that still works.

mod apply;
mod commit;
mod files;
mod mapping;
mod plan;
mod play_deltas;

use std::path::PathBuf;

use serde::Serialize;

use crate::database::IpodMediaKind;
use crate::device::{DatabaseFormat, Ipod};
use crate::error::{Error, Result};
use crate::models::{IpodFamily, SyncSupport};

pub use apply::apply_sync;
pub use plan::plan_sync;
pub use play_deltas::{PlayDelta, clear_play_counts, read_play_counts_for_sync_back};

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
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
    /// Cover art (JPEG or PNG). Artwork is not written to the iPod yet, so this is ignored.
    #[serde(skip)]
    pub artwork: Option<Vec<u8>>,
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
    /// Size of the source files of `add`. A prepared (transcoded) file can differ; the source is a
    /// safe estimate when transcoding shrinks, and is exact for plain copies.
    pub bytes_needed: u64,
    /// Whether `bytes_needed` plus working space for the database rewrite fits in the free space.
    /// Removed files are deleted only after the new database is committed, so they do not count.
    pub fits: bool,
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
    if ipod
        .model
        .as_ref()
        .is_some_and(|model| model.family == IpodFamily::Shuffle)
    {
        return refuse("iPod shuffles also need an iTunesSD database, which cannot be written yet");
    }
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

/// 16 hex digits, the form every iPod's FireWire GUID has.
fn parse_guid(guid: &str) -> Option<[u8; 8]> {
    if guid.len() != 16 {
        return None;
    }
    let mut bytes = [0u8; 8];
    for (byte, pair) in bytes.iter_mut().zip(guid.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(bytes)
}
