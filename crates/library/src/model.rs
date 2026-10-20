use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

use crate::loudness::TrackLoudness;
use crate::smart::SmartRules;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Music,
    Podcast,
    Audiobook,
}

impl ToSql for MediaKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(*self as i64))
    }
}

impl FromSql for MediaKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_i64()? {
            0 => Ok(Self::Music),
            1 => Ok(Self::Podcast),
            2 => Ok(Self::Audiobook),
            other => Err(FromSqlError::OutOfRange(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    Name,
    Time,
    Artist,
    Album,
    AlbumArtist,
    Genre,
    Year,
    Rating,
    Plays,
    LastPlayed,
    DateAdded,
    Kind,
    Bitrate,
    SampleRate,
    Size,
    /// Position within the playlist being listed; elsewhere it falls back to Artist.
    PlaylistOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackQuery {
    pub kind: Option<MediaKind>,
    pub search: Option<String>,
    pub sort: SortKey,
    pub descending: bool,
    /// Restricts the list to these track ids, e.g. a device playlist's members.
    pub ids: Option<Vec<i64>>,
    /// Restricts the list to one playlist's contents. Combines with the other filters.
    pub playlist: Option<i64>,
}

impl Default for TrackQuery {
    fn default() -> Self {
        Self {
            kind: Some(MediaKind::Music),
            search: None,
            sort: SortKey::Artist,
            descending: false,
            ids: None,
            playlist: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub path: String,
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
    pub duration_ms: u64,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub codec: String,
    pub file_size: u64,
    pub date_added: i64,
    pub date_modified: i64,
    pub play_count: u32,
    pub last_played: Option<i64>,
    pub skip_count: u32,
    pub rating: u8,
    pub kind: MediaKind,
    pub compilation: bool,
    /// The song checkbox: unchecked songs are skipped by playback and by "only checked" smart playlists.
    pub enabled: bool,
    /// The file is only a placeholder in the cloud (iCloud Drive's "Optimize Mac
    /// Storage"). Its details come from the file's name and folders until it is
    /// downloaded and the scan or `refresh_cloud_status` reads its tags.
    pub in_cloud: bool,
    /// The file was not found at `path` when last checked (`check_missing`); a
    /// rescan that finds it clears the flag, `relink` points the track elsewhere.
    pub missing: bool,
    /// How loud the track is, once known: read from its tags at scan time or
    /// measured by the app.
    pub loudness: Option<TrackLoudness>,
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub count: u64,
    pub total_duration_ms: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artwork {
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub scanned: u64,
    pub added: u64,
    pub current_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub scanned: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub failed: Vec<(String, String)>,
}

/// A song from another library (e.g. Apple Music) to look for in ours.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalTrack {
    pub external_id: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub duration_ms: Option<u64>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchQuality {
    SameFile,
    Exact,
    Close,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMatch {
    pub external_id: String,
    pub track_id: i64,
    pub quality: MatchQuality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistKind {
    Regular,
    Smart,
    Folder,
}

impl ToSql for PlaylistKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(*self as i64))
    }
}

impl FromSql for PlaylistKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_i64()? {
            0 => Ok(Self::Regular),
            1 => Ok(Self::Smart),
            2 => Ok(Self::Folder),
            other => Err(FromSqlError::OutOfRange(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub kind: PlaylistKind,
    pub parent_id: Option<i64>,
    /// Index among its siblings.
    pub position: u32,
    pub smart: Option<SmartRules>,
    /// Rows a listing would show: duplicates count, folders count their subtree once.
    pub track_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    /// Stable for as long as the album's artist and name tags are.
    pub key: String,
    pub album: String,
    pub album_artist: String,
    pub year: Option<i32>,
    pub genre: String,
    pub track_count: u32,
    pub duration_ms: u64,
    /// The album's first-added track; the one to ask `artwork()` about.
    pub artwork_track_id: i64,
    pub date_added: i64,
    pub compilation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistSummary {
    pub key: String,
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenreSummary {
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerSummary {
    pub name: String,
    pub track_count: u32,
}

/// Play data from another library for a track already in ours.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalStats {
    pub play_count: u32,
    pub skip_count: u32,
    pub last_played: Option<i64>,
    pub last_skipped: Option<i64>,
    /// 0..=100, stored as given (capped at 100); the importer should round half stars.
    pub rating: u8,
    pub date_added: Option<i64>,
    pub enabled: bool,
}

/// Metadata that wins over what a file's tags say. Stored with the track, so a
/// rescan re-applies it instead of reverting to the tags.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackOverrides {
    pub kind: Option<MediaKind>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub date_added: Option<i64>,
    pub year: Option<i32>,
}

/// A themed, generated playlist. The key is stable for as long as the cluster it
/// came from exists, so the UI can remember which mix is selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mix {
    pub key: String,
    pub name: String,
    pub track_ids: Vec<i64>,
    /// Up to four tracks from distinct albums, for a collage.
    pub artwork_track_ids: Vec<i64>,
}
