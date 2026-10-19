use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

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
}

impl Default for TrackQuery {
    fn default() -> Self {
        Self {
            kind: Some(MediaKind::Music),
            search: None,
            sort: SortKey::Artist,
            descending: false,
            ids: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
