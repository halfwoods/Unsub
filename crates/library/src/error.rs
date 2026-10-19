use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("tag error: {0}")]
    Tags(#[from] lofty::error::FileParseError),
    #[error("unsupported audio format")]
    UnsupportedFormat,
    #[error("path is not valid UTF-8: {}", .0.display())]
    NonUtf8Path(PathBuf),
    #[error("rating must be 0..=100 in steps of 20, got {0}")]
    InvalidRating(u8),
    #[error("no track with id {0}")]
    TrackNotFound(i64),
    #[error("database schema version {found} is newer than the supported version {supported}")]
    SchemaTooNew { found: u32, supported: u32 },
}

pub type Result<T> = std::result::Result<T, Error>;
