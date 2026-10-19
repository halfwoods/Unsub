use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("stored or received JSON is unreadable: {0}")]
    Json(#[from] serde_json::Error),
    #[error("file watcher error: {0}")]
    Watch(#[from] notify::Error),
    #[error("http error: {0}")]
    Http(#[from] ureq::Error),
    #[error("no pending candidate with id {0}")]
    CandidateNotFound(i64),
    #[error("archive entry {0:?} would extract outside the destination")]
    UnsafeArchive(String),
    #[error("download is larger than the {limit}-byte extraction limit")]
    TooLarge { limit: u64 },
    #[error("download holds more than the {limit} files extraction allows")]
    TooManyEntries { limit: usize },
    #[error("{} holds no audio files", .0.display())]
    NoAudio(PathBuf),
    #[error("ledger schema version {found} is newer than the supported version {supported}")]
    SchemaTooNew { found: u32, supported: u32 },
}

pub type Result<T> = std::result::Result<T, Error>;
