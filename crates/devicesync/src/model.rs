use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A library track to put on the device. The tags only name the file on the device; the audio comes
/// from `source`, through the `prepare` step of [`apply`](crate::apply).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncItem {
    /// The app's id for the track. It must stay the same for the same track across syncs: it is
    /// how a later sync finds the copy on the device to update or remove.
    pub library_id: i64,
    pub source: PathBuf,
    pub album_artist: String,
    pub album: String,
    pub title: String,
    pub disc: Option<u32>,
    pub track: Option<u32>,
    /// Bytes of `source`.
    pub size: u64,
    /// Modification time of `source`, in Unix seconds. A change to it (or to `size`) makes the
    /// next sync copy the track again.
    pub mtime: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlaylist {
    pub name: String,
    /// `library_id`s in playlist order. Tracks that did not make it onto the device are left out.
    pub items: Vec<i64>,
}
