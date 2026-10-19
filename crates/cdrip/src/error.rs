use std::{io, path::PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// This platform or format combination has no implementation yet.
    #[error("not supported on this platform: {0}")]
    Unsupported(&'static str),
    /// The rip was stopped through its cancel flag; completed tracks stay on disk.
    #[error("the rip was cancelled")]
    Cancelled,
    #[error("unreadable disc: {0}")]
    Disc(String),
    #[error("could not read {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("network request failed: {0}")]
    Network(String),
    #[error("unexpected response from {service}: {detail}")]
    Response {
        service: &'static str,
        detail: String,
    },
    #[error("encoding failed: {0}")]
    Encode(String),
    #[error("tagging failed: {0}")]
    Tag(#[from] lofty::error::FileEncodingError),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
