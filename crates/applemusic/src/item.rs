use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One entry of the user's Apple Music / iTunes library, from either source.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MusicLibraryItem {
    /// 16 uppercase hex characters, the same value the exported XML carries.
    pub persistent_id: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub duration_ms: Option<u64>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<i32>,
    /// iTunes' own description, such as "Apple Music AAC audio file" or "MPEG audio file".
    pub kind: String,
    pub media: MediaType,
    pub ownership: Ownership,
    /// Set only when a file exists at that path on this computer.
    pub location: Option<PathBuf>,
    pub play_count: u32,
    /// 0..=100, in steps of 20 per star.
    pub rating: u8,
    /// Unix seconds.
    pub date_added: Option<i64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaType {
    Music,
    Podcast,
    Audiobook,
    Video,
    Other,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Ownership {
    /// A file on this computer that the user owns: ripped, bought or matched, and downloaded.
    /// Pre-2009 FairPlay purchases count even though they only play in Apple's software.
    LocalFile,
    /// Bought from the iTunes Store but not downloaded here; a free re-download.
    PurchasedInCloud,
    /// Uploaded or matched to iCloud Music Library, not downloaded here.
    UploadedInCloud,
    /// Streamed or downloaded through an Apple Music subscription; not owned.
    Subscription,
    Unknown,
}
