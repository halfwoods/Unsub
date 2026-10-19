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
