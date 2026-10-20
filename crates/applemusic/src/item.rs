use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A whole library: its songs and the playlists that refer to them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MusicLibrary {
    pub items: Vec<MusicLibraryItem>,
    pub playlists: Vec<MusicPlaylist>,
}

impl MusicLibrary {
    /// Playlists whose parent was dropped as a system playlist move to the top level, so no
    /// parent id points at a playlist that is not in the result.
    pub(crate) fn new(items: Vec<MusicLibraryItem>, mut playlists: Vec<MusicPlaylist>) -> Self {
        let kept: HashSet<String> = playlists
            .iter()
            .map(|playlist| playlist.persistent_id.clone())
            .collect();
        for playlist in &mut playlists {
            if playlist
                .parent_persistent_id
                .as_ref()
                .is_some_and(|parent| !kept.contains(parent))
            {
                playlist.parent_persistent_id = None;
            }
        }
        Self { items, playlists }
    }
}

/// A user playlist or playlist folder. The master "Library" playlist and Apple's own system
/// playlists (Music, Movies, Purchased, Recently Added and the rest) are left out.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MusicPlaylist {
    /// 16 uppercase hex characters.
    pub persistent_id: String,
    pub name: String,
    /// The folder this playlist sits in; `None` at the top level.
    pub parent_persistent_id: Option<String>,
    pub kind: PlaylistKind,
    /// `MusicLibraryItem::persistent_id` of each song, in playlist order. A song can repeat.
    pub item_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistKind {
    Regular,
    /// Rule-based. The rules are an opaque binary format, so only the members it had when the
    /// library was read are kept, as `item_ids`.
    Smart,
    /// Holds other playlists and has no songs of its own.
    Folder,
    /// A saved Genius playlist or Genius Mix. Only the system library reports it; the exported
    /// XML is read without looking for it, so its Genius playlists come out as `Regular`.
    Genius,
}

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
    /// The path the library records for the file, kept even when the file is gone so it can be
    /// found again; see `file_present`.
    pub location: Option<PathBuf>,
    /// Whether a file exists at `location` right now.
    #[serde(default)]
    pub file_present: bool,
    pub play_count: u32,
    #[serde(default)]
    pub skip_count: u32,
    /// 0..=100, in steps of 20 per star.
    pub rating: u8,
    /// Unix seconds.
    pub date_added: Option<i64>,
    /// Unix seconds; `None` if never played.
    #[serde(default)]
    pub last_played: Option<i64>,
    /// Unix seconds; `None` if never skipped.
    #[serde(default)]
    pub last_skipped: Option<i64>,
    /// False when the user has unchecked the song in the library. Absent in snapshots saved before
    /// the field existed, which are read as enabled.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
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
    /// The library points at a file on this computer that is not there: moved, deleted, or on a
    /// volume that is not mounted. The user owns it; the file only has to be found.
    MissingFile,
    /// Streamed or downloaded through an Apple Music subscription; not owned.
    Subscription,
    Unknown,
}

fn enabled_by_default() -> bool {
    true
}
