use std::path::PathBuf;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::model::{Export, Play};
use crate::read::{file_name, json, text};

/// ListenBrainz's export ZIP: `listens/<year>/<month>.jsonl`, one listen per line. The same
/// listens as the API gives them (a JSON array, or `{"payload": {"listens": [...]}}`) are read as
/// well. Last.fm history arrives here too, once ListenBrainz has imported it.
pub(crate) fn read(files: &[PathBuf]) -> Result<Export> {
    let mut export = Export::default();
    for file in files {
        let name = file_name(file);
        let listens = if name.ends_with(".jsonl") {
            let mut listens = Vec::new();
            for line in text(file)?.lines().filter(|line| !line.trim().is_empty()) {
                listens.push(serde_json::from_str(line).map_err(|source| Error::Json {
                    file: file.clone(),
                    source,
                })?);
            }
            listens
        } else if name.ends_with(".json") {
            json::<Document>(file)?.into_listens()
        } else {
            continue;
        };
        export
            .plays
            .extend(listens.into_iter().filter_map(Listen::into_play));
    }
    Ok(export)
}

/// Feedback, pinned recordings and `user.json` share the folder and the extension; they have no
/// `track_metadata`, so they come out as listens with nothing in them and are dropped.
#[derive(Deserialize)]
struct Listen {
    listened_at: Option<i64>,
    track_metadata: Option<TrackMetadata>,
}

#[derive(Deserialize)]
struct TrackMetadata {
    artist_name: Option<String>,
    track_name: Option<String>,
    release_name: Option<String>,
}

impl Listen {
    fn into_play(self) -> Option<Play> {
        let metadata = self.track_metadata?;
        let (title, artist) = (metadata.track_name?, metadata.artist_name?);
        if title.trim().is_empty() || artist.trim().is_empty() {
            return None;
        }
        Some(Play {
            at: self.listened_at?,
            title,
            artist,
            album: metadata
                .release_name
                .filter(|album| !album.trim().is_empty()),
            ms_played: None,
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Document {
    Listens(Vec<Listen>),
    Object {
        payload: Option<Payload>,
        listens: Option<Vec<Listen>>,
    },
}

#[derive(Deserialize)]
struct Payload {
    listens: Option<Vec<Listen>>,
}

impl Document {
    fn into_listens(self) -> Vec<Listen> {
        match self {
            Self::Listens(listens) => listens,
            Self::Object { payload, listens } => {
                listens.or_else(|| payload?.listens).unwrap_or_default()
            }
        }
    }
}
