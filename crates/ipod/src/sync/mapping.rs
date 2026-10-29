//! `iPod_Control/iTunes/Unsub.json`: which tracks and playlists on the iPod this app put
//! there. A re-sync recognises its own tracks by it, and only ever removes or edits those; anything
//! iTunes (or another tool) put on the iPod is not in the file and is left alone.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::device::itunes_dir;
use crate::error::{Error, Result};

const FILE_NAME: &str = "Unsub.json";
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Mapping {
    version: u32,
    pub(super) tracks: Vec<MappedTrack>,
    pub(super) playlists: Vec<MappedPlaylist>,
    /// Album art this app put in the ArtworkDB, so later syncs can reuse it and, once nothing
    /// refers to it, remove it. Absent in files written before artwork was supported.
    #[serde(default)]
    pub(super) artwork: Vec<MappedImage>,
    /// Additive video copies are separate from music ownership: an ordinary music sync must
    /// never remove them when they are absent from its selected library tracks.
    #[serde(default)]
    pub(super) video_copies: Vec<MappedVideo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MappedVideo {
    pub(super) sha256: String,
    pub(super) dbid: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MappedTrack {
    pub(super) library_id: i64,
    /// The iPod's persistent 64-bit track id, which survives the renumbering of 32-bit ids.
    pub(super) dbid: u64,
    /// The cover identity the library gave this track when its art was last handled, so a changed
    /// cover is noticed without loading it. Absent in files written before artwork was supported.
    #[serde(default)]
    pub(super) artwork_key: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MappedPlaylist {
    pub(super) name: String,
    pub(super) id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MappedImage {
    /// SHA-1 of the album name and cover bytes it was made from.
    pub(super) key: String,
    pub(super) image_id: u32,
}

impl Mapping {
    pub(super) fn new(
        tracks: Vec<MappedTrack>,
        playlists: Vec<MappedPlaylist>,
        artwork: Vec<MappedImage>,
    ) -> Self {
        Self {
            version: VERSION,
            tracks,
            playlists,
            artwork,
            video_copies: Vec::new(),
        }
    }

    pub(super) fn with_video_copies(mut self, videos: Vec<MappedVideo>) -> Self {
        self.video_copies = videos;
        self
    }

    pub(super) fn path(mount: &Path) -> PathBuf {
        itunes_dir(mount).join(FILE_NAME)
    }

    /// A missing file means nothing was synced yet. An unreadable one is an error: guessing would
    /// either duplicate every track or lose track of which ones are ours.
    pub(super) fn load(mount: &Path) -> Result<Self> {
        let path = Self::path(mount);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::new(Vec::new(), Vec::new(), Vec::new()));
            }
            Err(source) => return Err(Error::io(path, source)),
        };
        let mapping: Self = serde_json::from_slice(&bytes)
            .map_err(|error| Error::Mapping(format!("{}: {error}", path.display())))?;
        if mapping.version > VERSION {
            return Err(Error::Mapping(format!(
                "{} was written by a newer version of this app",
                path.display()
            )));
        }
        Ok(mapping)
    }

    pub(super) fn to_json(&self) -> Result<Vec<u8>> {
        serde_json::to_vec_pretty(self).map_err(|error| Error::Mapping(error.to_string()))
    }

    pub(super) fn dbid_by_library_id(&self) -> HashMap<i64, u64> {
        self.tracks
            .iter()
            .map(|track| (track.library_id, track.dbid))
            .collect()
    }

    pub(super) fn artwork_key_by_library_id(&self) -> HashMap<i64, Option<u64>> {
        self.tracks
            .iter()
            .map(|track| (track.library_id, track.artwork_key))
            .collect()
    }

    pub(super) fn library_id_by_dbid(&self) -> HashMap<u64, i64> {
        self.tracks
            .iter()
            .map(|track| (track.dbid, track.library_id))
            .collect()
    }
}
