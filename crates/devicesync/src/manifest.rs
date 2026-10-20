//! `.antistreaming/manifest.json`: which files on the device this app put there. A re-sync
//! recognises its own files by it, copies only what changed, and only ever deletes what it lists.
//! Anything else on the player is the user's and is never touched.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{Error, Result, Storage};

pub(crate) const PATH: &str = ".antistreaming/manifest.json";
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Manifest {
    version: u32,
    /// Counts the changes to `items` and `playlists`, so a plan made earlier can tell that the
    /// device has moved on since.
    pub revision: u64,
    pub items: BTreeMap<i64, Entry>,
    /// Playlist name to its path on the device.
    pub playlists: BTreeMap<String, String>,
    /// Files the sync in progress may have created without having recorded them yet. If the cable
    /// is pulled, the next sync deletes the ones that are not in `items`.
    pub pending: Vec<String>,
    /// Files that are no longer wanted and are being deleted. Listed first so a sync interrupted
    /// between recording the change and deleting the file still finishes the job.
    pub trash: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub path: String,
    /// Bytes of the file on the device, which differ from the source's after a conversion.
    pub size: u64,
    pub source_size: u64,
    pub source_mtime: i64,
}

impl Manifest {
    pub fn empty() -> Self {
        Self {
            version: VERSION,
            revision: 0,
            items: BTreeMap::new(),
            playlists: BTreeMap::new(),
            pending: Vec::new(),
            trash: Vec::new(),
        }
    }

    /// A missing manifest means nothing was synced yet. An unreadable one is an error.
    pub fn load(storage: &dyn Storage) -> Result<Self> {
        let bytes = match storage.read(PATH) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(Self::empty()),
            Err(source) => return Err(Error::io(PATH, source)),
        };
        let manifest: Self = serde_json::from_slice(&bytes)
            .map_err(|error| Error::Manifest(format!("{PATH}: {error}")))?;
        if manifest.version > VERSION {
            return Err(Error::Manifest(format!(
                "{PATH} was written by a newer version of this app"
            )));
        }
        Ok(manifest)
    }

    /// Writes the manifest atomically. `changed` says `items` or `playlists` differ from the last
    /// save, which advances the revision.
    pub fn save(&mut self, storage: &dyn Storage, changed: bool) -> Result<()> {
        if changed {
            self.revision += 1;
        }
        let bytes = serde_json::to_vec(self).map_err(|error| Error::Manifest(error.to_string()))?;
        storage
            .write(PATH, &bytes)
            .map_err(|source| Error::io(PATH, source))
    }

    /// Whether `lowercase_path` (compared case-insensitively, as FAT does) is one of our files.
    /// Scans every entry; to ask about many paths use [`Manifest::owned_paths`].
    pub fn owns(&self, lowercase_path: &str) -> bool {
        self.items
            .values()
            .any(|entry| entry.path.to_lowercase() == lowercase_path)
            || self
                .playlists
                .values()
                .any(|path| path.to_lowercase() == lowercase_path)
    }

    /// Every path we own, lower-cased.
    pub fn owned_paths(&self) -> HashSet<String> {
        self.items
            .values()
            .map(|entry| entry.path.to_lowercase())
            .chain(self.playlists.values().map(|path| path.to_lowercase()))
            .collect()
    }

    /// Which track holds each lower-cased track path.
    pub fn track_owners(&self) -> HashMap<String, i64> {
        self.items
            .iter()
            .map(|(id, entry)| (entry.path.to_lowercase(), *id))
            .collect()
    }
}
