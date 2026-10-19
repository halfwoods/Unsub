use std::path::PathBuf;

use serde::{Serialize, Serializer};

use crate::device::{Ipod, database_path};
use crate::error::Result;
use crate::{itunesdb, play_counts};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Database {
    pub tracks: Vec<IpodTrack>,
    pub playlists: Vec<IpodPlaylist>,
    pub usage: Usage,
}

/// Bytes per media kind, for the iTunes-style capacity bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub audio_bytes: u64,
    pub video_bytes: u64,
    pub podcast_bytes: u64,
    pub audiobook_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpodTrack {
    pub id: u32,
    /// 64-bit values exceed JavaScript's safe integer range, so they are serialized as strings.
    #[serde(serialize_with = "as_decimal_string")]
    pub dbid: u64,
    /// Absolute path of the media file under the mount; empty if the database stored no location.
    pub path: PathBuf,
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
    pub file_size: u64,
    pub play_count: u32,
    /// Unix seconds.
    pub last_played: Option<i64>,
    pub skip_count: u32,
    /// 0 to 100, in steps of 20 per star.
    pub rating: u8,
    /// Unix seconds.
    pub date_added: Option<i64>,
    pub media_kind: IpodMediaKind,
    /// The type string iTunes stored ("MPEG audio file") or, for flash iPods, the four-letter code.
    pub file_type: String,
    pub compilation: bool,
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IpodMediaKind {
    #[default]
    Music,
    Podcast,
    Audiobook,
    Video,
    Other,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpodPlaylist {
    /// 64-bit, serialized as a string like [`IpodTrack::dbid`].
    #[serde(serialize_with = "as_decimal_string")]
    pub id: u64,
    pub name: String,
    pub is_master: bool,
    pub is_podcasts: bool,
    pub is_smart: bool,
    /// Ids of member tracks in playlist order. Smart playlists list the tracks the iPod last
    /// evaluated; their rules are not interpreted.
    pub track_ids: Vec<u32>,
}

fn as_decimal_string<S: Serializer>(
    value: &u64,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

/// Parses the iPod's database and folds in the plays, skips and ratings the iPod recorded since the
/// last sync (`iPod_Control/iTunes/Play Counts`). A missing or unreadable Play Counts file is
/// ignored, as in libgpod, because the library is still correct without it.
pub fn read_database(ipod: &Ipod) -> Result<Database> {
    let path = database_path(&ipod.mount_path, ipod.database);
    let data = itunesdb::load(&path, ipod.database)?;
    let (mut tracks, playlists) = itunesdb::parse(&data, &ipod.mount_path)?;
    play_counts::merge(&ipod.mount_path, &mut tracks);
    let usage = Usage::of(&tracks);
    Ok(Database {
        tracks,
        playlists,
        usage,
    })
}

impl Usage {
    fn of(tracks: &[IpodTrack]) -> Self {
        let mut usage = Self::default();
        for track in tracks {
            let bucket = match track.media_kind {
                IpodMediaKind::Music => &mut usage.audio_bytes,
                IpodMediaKind::Video => &mut usage.video_bytes,
                IpodMediaKind::Podcast => &mut usage.podcast_bytes,
                IpodMediaKind::Audiobook => &mut usage.audiobook_bytes,
                IpodMediaKind::Other => continue,
            };
            *bucket += track.file_size;
        }
        usage
    }
}
