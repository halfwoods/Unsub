//! Replacing a file on the iPod so that a failure or a pulled cable at any point leaves either the
//! old file or the new one, never half of each: write beside it, flush to the device, then rename.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const TEMP_SUFFIX: &str = ".antistreaming-tmp";
pub(super) const BACKUP_SUFFIX: &str = ".antistreaming-backup";
const ORIGINAL_SUFFIX: &str = ".antistreaming-original";

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// New contents written and flushed next to their destination, not yet visible under its name.
pub(super) struct Staged {
    temp: PathBuf,
    destination: PathBuf,
}

impl Staged {
    pub(super) fn write(destination: &Path, bytes: &[u8]) -> Result<Self> {
        let temp = beside(destination, TEMP_SUFFIX);
        let written = File::create(&temp).and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        });
        if let Err(source) = written {
            let _ = fs::remove_file(&temp);
            return Err(Error::io(temp, source));
        }
        Ok(Self {
            temp,
            destination: destination.to_path_buf(),
        })
    }

    /// Atomically replaces the destination. `rename` over an existing file is atomic on every
    /// filesystem iPods use and, unlike remove-then-rename, never leaves the name missing.
    pub(super) fn commit(self) -> Result<()> {
        fs::rename(&self.temp, &self.destination)
            .map_err(|source| Error::io(&self.destination, source))?;
        sync_directory(&self.destination);
        Ok(())
    }

    pub(super) fn discard(self) {
        let _ = fs::remove_file(&self.temp);
    }
}

/// Copies the current database aside before it is replaced. The `.antistreaming-backup` copy is the
/// database as it was before the latest sync; `.antistreaming-original` is kept once, from before
/// the first sync, so a bad sync can never overwrite the only good copy.
pub(super) fn keep_backups(database: &Path) -> Result<()> {
    let backup = beside(database, BACKUP_SUFFIX);
    copy_and_flush(database, &backup)?;
    let original = beside(database, ORIGINAL_SUFFIX);
    if !original.exists() {
        copy_and_flush(database, &original)?;
    }
    Ok(())
}

fn copy_and_flush(from: &Path, to: &Path) -> Result<()> {
    let copied = fs::copy(from, to).and_then(|_| File::open(to)?.sync_all());
    copied.map_err(|source| Error::io(to, source))
}

/// Makes a rename durable. Not every filesystem supports flushing a directory and the file data is
/// already flushed, so a failure here is not worth failing the sync for.
pub(super) fn sync_directory(entry: &Path) {
    #[cfg(unix)]
    if let Some(directory) = entry.parent().and_then(|dir| File::open(dir).ok()) {
        let _ = directory.sync_all();
    }
    #[cfg(not(unix))]
    let _ = entry;
}
