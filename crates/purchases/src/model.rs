use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where a download came from. Only a label: nothing is ever refused for being `Unknown`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Store {
    Bandcamp,
    Qobuz,
    SevenDigital,
    Hdtracks,
    #[default]
    Unknown,
}

impl Store {
    /// The spelling kept in the ledger, independent of serde's wire format.
    pub(crate) fn as_db(self) -> &'static str {
        match self {
            Self::Bandcamp => "bandcamp",
            Self::Qobuz => "qobuz",
            Self::SevenDigital => "sevendigital",
            Self::Hdtracks => "hdtracks",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn from_db(text: &str) -> Self {
        match text {
            "bandcamp" => Self::Bandcamp,
            "qobuz" => Self::Qobuz,
            "sevendigital" => Self::SevenDigital,
            "hdtracks" => Self::Hdtracks,
            _ => Self::Unknown,
        }
    }
}

/// A finished download waiting for the user to accept or ignore it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: i64,
    /// The zip archive or the folder.
    pub path: PathBuf,
    pub store: Store,
    pub artist: String,
    pub album: String,
    pub track_count: u32,
    /// Size on disk: the zip itself, or every file in the folder.
    pub bytes: u64,
    /// Upper-case extensions of the audio inside, sorted: `["FLAC"]`.
    pub formats: Vec<String>,
    /// Unix seconds.
    pub found_at: i64,
}

/// An accepted download, as shown in the "Purchased" list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Purchase {
    pub id: i64,
    pub store: Store,
    pub artist: String,
    pub album: String,
    /// The audio and artwork written into the library folder, ready to import.
    pub files: Vec<PathBuf>,
    /// Unix seconds.
    pub imported_at: i64,
    /// The zip or folder it was taken from, which is left where it was.
    pub source: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreLink {
    /// "Bandcamp", "Qobuz", "iTunes", or the site's own name for stores not listed.
    pub store: String,
    pub url: String,
}
