//! `iPod_Control/iTunes/AntiStreaming.json`: which tracks and playlists on the iPod this app put
//! there. A re-sync recognises its own tracks by it, and only ever removes or edits those; anything
//! iTunes (or another tool) put on the iPod is not in the file and is left alone.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::device::itunes_dir;
use crate::error::{Error, Result};

const FILE_NAME: &str = "AntiStreaming.json";
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Mapping {
    version: u32,
    pub(super) tracks: Vec<MappedTrack>,
    pub(super) playlists: Vec<MappedPlaylist>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MappedTrack {
    pub(super) library_id: i64,
    /// The iPod's persistent 64-bit track id, which survives the renumbering of 32-bit ids.
    pub(super) dbid: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MappedPlaylist {
    pub(super) name: String,
    pub(super) id: u64,
}

impl Mapping {
    pub(super) fn new(tracks: Vec<MappedTrack>, playlists: Vec<MappedPlaylist>) -> Self {
        Self {
            version: VERSION,
            tracks,
            playlists,
        }
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
                return Ok(Self::new(Vec::new(), Vec::new()));
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

    pub(super) fn library_id_by_dbid(&self) -> HashMap<u64, i64> {
        self.tracks
            .iter()
            .map(|track| (track.dbid, track.library_id))
            .collect()
    }
}
