use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `path` is a device path (or the local file involved) as a string.
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("{0}")]
    Unsupported(&'static str),
    /// The system refused to eject the device, usually because something still has a file open.
    #[error("could not eject {device}: {message}")]
    Eject { device: String, message: String },
    /// The sync was stopped through its cancel flag. Everything copied before that is recorded.
    #[error("the sync was cancelled")]
    Cancelled,
    #[error("invalid device profile: {0}")]
    InvalidProfile(String),
    /// The sync bookkeeping on the device could not be read. Guessing would either duplicate every
    /// track or lose track of which ones are ours, so nothing is touched.
    #[error("unreadable sync manifest: {0}")]
    Manifest(String),
    #[error("not enough space on the device: {needed} bytes needed, {available} available")]
    NotEnoughSpace { needed: u64, available: u64 },
    /// The device or the library no longer matches the plan; nothing was written.
    #[error("{0}")]
    Conflict(String),
}

impl Error {
    pub(crate) fn io(path: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
