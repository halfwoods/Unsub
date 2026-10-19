use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{} is not an iPod volume (no iTunesDB or iTunesCDB found)", .0.display())]
    NotAnIpod(PathBuf),
    #[error("corrupt iPod database at offset {offset:#x}: {reason}")]
    Corrupt { offset: usize, reason: String },
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not eject {}: {message}", .mount.display())]
    Eject { mount: PathBuf, message: String },
    #[error("{0}")]
    Unsupported(&'static str),
    #[error("cannot sync to this iPod: {0}")]
    SyncRefused(String),
    #[error("not enough space on the iPod: {needed} bytes needed, {available} available")]
    InsufficientSpace { needed: u64, available: u64 },
    /// The iPod no longer matches the plan or the library; nothing was written.
    #[error("{0}")]
    Conflict(String),
    #[error("unreadable sync bookkeeping: {0}")]
    Mapping(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn corrupt(offset: usize, reason: impl Into<String>) -> Self {
        Self::Corrupt {
            offset,
            reason: reason.into(),
        }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
