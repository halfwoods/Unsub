//! A read-only check that this crate can rewrite a real iPod's database without losing anything,
//! for use on a device before trusting a sync to it. Nothing here can write to the device: the
//! comparison takes the database as bytes and builds its rewrite in memory.

use std::fs;

use serde::Serialize;

use crate::database::{IpodPlaylist, IpodTrack};
use crate::device::{Ipod, database_path};
use crate::error::Result;
use crate::itunesdb;

/// Whether the checksum iTunes stored in the database is the one this crate computes for the
/// iPod's FireWire GUID. A match on a real iTunes-written database proves the HASH58 code and the
/// GUID handling are right for that device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChecksumCheck {
    Verified,
    Mismatch,
    /// The database carries no checksum (older iPods).
    Absent,
    /// A scheme this crate cannot write (2 is HASH72, 3 is HASHAB).
    OtherScheme(u16),
    /// The database has a HASH58 checksum but the iPod's GUID is unknown.
    NoFirewireGuid,
}

/// What the read-only look at `iPod_Control/Artwork/ArtworkDB` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtworkCheck {
    /// The iPod has no ArtworkDB.
    Absent,
    /// It exists but this crate cannot read it, so a sync that adds covers would be refused.
    Unreadable(String),
    #[serde(rename_all = "camelCase")]
    Checked {
        images: u64,
        thumbnails: u64,
        /// Thumbnails pointing outside their `.ithmb` file; a fault of the device's own files.
        missing_thumbnails: u64,
        /// Whether rewriting the ArtworkDB with nothing added reproduces it byte for byte.
        identical: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionSize {
    /// The mhsd type: 1 tracks, 2 and 3 playlists, 4 albums, 8 artists, anything else is copied.
    pub kind: u32,
    pub original_bytes: u64,
    pub rewritten_bytes: u64,
    /// Whether the rewrite builds this section afresh (true) or copies it as it was (false).
    pub rebuilt: bool,
}

/// A field of a track or playlist as the reader sees it, before and after the rewrite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldDifference {
    pub field: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordDifference {
    /// `track 52` or `playlist 4660`.
    pub record: String,
    pub name: String,
    pub fields: Vec<FieldDifference>,
}

/// A 4-byte word of a record header that the rewrite changed, over all records of that kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderChange {
    /// `mhbd`, `mhit` or `mhyp`.
    pub record: String,
    pub offset: usize,
    pub records_changed: u64,
    pub example_before: u32,
    pub example_after: u32,
    /// Lengths, counts and ids the rewrite recomputes on purpose.
    pub expected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkCount {
    /// `mhit`, `mhod:52`, `unexpected:abcd` and so on.
    pub chunk: String,
    pub original: u64,
    pub rewritten: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundtripReport {
    pub original_bytes: u64,
    pub rewritten_bytes: u64,
    pub tracks: u64,
    pub playlists: u64,
    pub checksum: ChecksumCheck,
    pub artwork: ArtworkCheck,
    pub sections: Vec<SectionSize>,
    pub header_changes: Vec<HeaderChange>,
    /// Tracks whose fields, as the reader sees them, differ after the rewrite.
    pub track_differences: Vec<RecordDifference>,
    pub playlist_differences: Vec<RecordDifference>,
    /// Tracks and playlists whose mhods (the strings and settings attached to them) changed,
    /// as `record: kinds` lines. Empty when every mhod was copied exactly.
    pub mhod_differences: Vec<String>,
    /// Chunk kinds whose number differs between the original and the rewrite.
    pub chunk_counts: Vec<ChunkCount>,
}

impl RoundtripReport {
    /// True when the rewrite lost or changed nothing it should have kept: no reader-visible field
    /// differs, every mhod was copied, only expected header words changed, and the only chunks that
    /// appear or disappear are derived data the writer always rebuilds from the tracks (the album
    /// and artist lists and the master playlist's sort indexes).
    pub fn is_lossless(&self) -> bool {
        let rebuilt_only = |count: &ChunkCount| {
            matches!(
                count.chunk.as_str(),
                "mhia"
                    | "mhii"
                    | "mhod:200"
                    | "mhod:201"
                    | "mhod:202"
                    | "mhod:300"
                    | "mhod:52"
                    | "mhod:53"
            )
        };
        self.track_differences.is_empty()
            && self.playlist_differences.is_empty()
            && self.mhod_differences.is_empty()
            && self.header_changes.iter().all(|change| change.expected)
            && self.chunk_counts.iter().all(rebuilt_only)
            && self.checksum != ChecksumCheck::Mismatch
            && matches!(
                self.artwork,
                ArtworkCheck::Absent
                    | ArtworkCheck::Checked {
                        identical: true,
                        ..
                    }
            )
    }
}

/// Parses the iPod's database, rewrites it in memory with the sync writer, parses that, and reports
/// every difference. The device is only read.
pub fn verify_roundtrip(ipod: &Ipod) -> Result<RoundtripReport> {
    let path = database_path(&ipod.mount_path, ipod.database);
    let original = itunesdb::load(&path, ipod.database)?;
    let firewire_id = ipod
        .firewire_guid
        .as_deref()
        .and_then(crate::sync::parse_guid);
    let artwork_path = itunesdb::artwork::database_path(&ipod.mount_path);
    let artwork_db = fs::read(&artwork_path).ok();
    itunesdb::roundtrip::compare(
        &original,
        artwork_db.as_deref(),
        &ipod.mount_path,
        firewire_id.as_ref(),
    )
}

pub(crate) fn describe_track(track: &IpodTrack) -> (String, String) {
    (format!("track {}", track.id), track.title.clone())
}

pub(crate) fn describe_playlist(playlist: &IpodPlaylist) -> (String, String) {
    (format!("playlist {}", playlist.id), playlist.name.clone())
}
