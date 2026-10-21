use std::path::PathBuf;

use crate::model::Service;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{} isn't readable JSON: {source}", file.display())]
    Json {
        file: PathBuf,
        source: serde_json::Error,
    },
    #[error(
        "No playlists or plays from {} were found there. Choose the file, folder or ZIP from the export you downloaded.",
        .0.name()
    )]
    NothingFound(Service),
}

pub type Result<T> = std::result::Result<T, Error>;
