//! Response shapes that differ from the model types: the older "directory" forms of albums and
//! songs, and the parents that carry their children.

use serde::Serialize;

use crate::model::{Album, Artist, Playlist, Song};

/// A song as the `Child` type, which adds the fixed fields the model leaves out.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SongEntry<'a> {
    #[serde(flatten)]
    song: &'a Song,
    is_dir: bool,
    is_video: bool,
    #[serde(rename = "type")]
    kind: &'static str,
}

impl<'a> SongEntry<'a> {
    pub fn new(song: &'a Song) -> Self {
        Self {
            song,
            is_dir: false,
            is_video: false,
            kind: "music",
        }
    }

    pub fn all(songs: &'a [Song]) -> Vec<Self> {
        songs.iter().map(Self::new).collect()
    }
}

/// An album as a directory `Child`, for the non-ID3 endpoints (`getAlbumList`, `search2`). It keeps
/// the ID3 fields too, so a client that reads either set finds what it expects.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDir<'a> {
    #[serde(flatten)]
    album: &'a Album,
    is_dir: bool,
    title: &'a str,
    parent: Option<&'a str>,
}

impl<'a> AlbumDir<'a> {
    pub fn all(albums: &'a [Album]) -> Vec<Self> {
        albums
            .iter()
            .map(|album| Self {
                album,
                is_dir: true,
                title: &album.name,
                parent: album.artist_id.as_deref(),
            })
            .collect()
    }
}

#[derive(Serialize)]
pub struct ArtistDetail<'a> {
    #[serde(flatten)]
    pub artist: &'a Artist,
    pub album: &'a [Album],
}

#[derive(Serialize)]
pub struct AlbumDetail<'a> {
    #[serde(flatten)]
    pub album: &'a Album,
    pub song: Vec<SongEntry<'a>>,
}

#[derive(Serialize)]
pub struct PlaylistDetail<'a> {
    #[serde(flatten)]
    pub playlist: &'a Playlist,
    pub entry: Vec<SongEntry<'a>>,
}

/// A folder as `getMusicDirectory` lists it: an artist's albums or an album's songs.
#[derive(Serialize)]
pub struct Directory<'a, C: Serialize> {
    pub id: &'a str,
    pub name: &'a str,
    pub parent: Option<&'a str>,
    pub child: Vec<C>,
}

/// The saved play queue.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayQueueDoc<'a> {
    pub current: Option<&'a str>,
    pub position: i64,
    pub username: &'a str,
    pub changed: String,
    pub changed_by: &'a str,
    pub entry: Vec<SongEntry<'a>>,
}

/// One song somebody is playing, for `getNowPlaying`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NowPlayingEntry<'a> {
    #[serde(flatten)]
    pub song: SongEntry<'a>,
    pub username: &'a str,
    pub minutes_ago: u64,
    pub player_name: &'a str,
}

/// Artists, albums and songs side by side, as `starred` and the search results hold them.
#[derive(Serialize)]
pub struct ByKind<A: Serialize, B: Serialize, S: Serialize> {
    pub artist: A,
    pub album: B,
    pub song: S,
}
