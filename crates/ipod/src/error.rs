use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{} is not an iPod volume (no iTunesDB, iTunesCDB or iPod device files found)", .0.display())]
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
    #[error("the iPod can't hold the selected music: it needs {} and has {} free", megabytes(.needed), megabytes(.available))]
    InsufficientSpace { needed: u64, available: u64 },
    /// The space ran out partway through, because of what else is on the iPod or because a
    /// converted song came out bigger than its source.
    #[error("the iPod ran out of space while copying: the next song needs {} and {} is free", megabytes(.needed), megabytes(.available))]
    OutOfSpace { needed: u64, available: u64 },
    /// The iPod no longer matches the plan or the library; nothing was written.
    #[error("{0}")]
    Conflict(String),
    #[error("unreadable sync bookkeeping: {0}")]
    Mapping(String),
    /// The iPod was short of space, so the tracks being removed were deleted first; then the sync
    /// stopped. The device is valid and has fewer tracks than before, never more.
    #[error("removed {removed} track(s) to make room, then the sync failed: {source}")]
    AfterRemovals { removed: usize, source: Box<Error> },
    /// A checkpoint had already been committed when the sync failed: the iPod has the songs added
    /// up to it and a valid database, and the next sync carries on from there.
    #[error("added {added} song(s) to the iPod, then the sync failed: {source}")]
    Partial { added: usize, source: Box<Error> },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Binary megabytes, or gigabytes from 1024 of them, as the app's own size labels read.
fn megabytes(bytes: &u64) -> String {
    let mb = *bytes as f64 / f64::from(1 << 20);
    if mb < 1024.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.2} GB", mb / 1024.0)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_out_of_space_says_how_much_in_readable_sizes() {
        let short = Error::InsufficientSpace {
            needed: 12 << 30,
            available: 3 << 29,
        };

        assert_eq!(
            short.to_string(),
            "the iPod can't hold the selected music: it needs 12.00 GB and has 1.50 GB free"
        );
        assert_eq!(megabytes(&(5 << 19)), "2.5 MB");
    }
}
