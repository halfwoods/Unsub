//! The file operations a sync needs, so the same planning and applying runs on a mounted volume and
//! on an MTP device.

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    ops::ControlFlow,
    path::{Component, Path, PathBuf},
};

use crate::{Device, DeviceRoot, Profile, Result, mtp};

/// Marks a half-written file. The leading dot hides it from players that scan while a sync runs.
pub(crate) const TEMP_SUFFIX: &str = ".as-part";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    /// Relative to the device root, `/` separated.
    pub path: String,
    pub size: u64,
}

/// A device's file system as seen by a sync. Paths are relative to the device root, `/` separated,
/// with no leading slash. Implementations must keep `put` and `write` atomic: the destination holds
/// either what was there before or the complete new content, never a prefix of it.
pub trait Storage {
    /// Every file below `dir`, however deep. A missing folder is empty.
    fn list(&self, dir: &str) -> io::Result<Vec<StoredFile>>;

    fn read(&self, path: &str) -> io::Result<Option<Vec<u8>>>;

    /// Creates or replaces `path` with `data`.
    fn write(&self, path: &str, data: &[u8]) -> io::Result<()>;

    /// Copies a local file to `dest`, creating folders as needed, and returns the bytes copied.
    /// `progress` gets the running total and can stop the copy by returning `Break`, which removes
    /// the partial file and fails with [`io::ErrorKind::Interrupted`].
    fn put(
        &self,
        source: &Path,
        dest: &str,
        progress: &mut dyn FnMut(u64) -> ControlFlow<()>,
    ) -> io::Result<u64>;

    /// Deletes a file; one that is already gone is not an error.
    fn remove(&self, path: &str) -> io::Result<()>;

    /// Removes `dir` if it holds nothing. Purely cosmetic, so it never fails.
    fn remove_dir_if_empty(&self, dir: &str);

    fn free_bytes(&self) -> io::Result<u64>;
}

/// Where a sync goes: a storage and the profile that says how to lay files out on it.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub storage: &'a dyn Storage,
    pub profile: &'a Profile,
}

/// Opens the storage a device's root points at.
pub fn open(device: &Device) -> Result<Box<dyn Storage>> {
    match &device.root {
        DeviceRoot::Path(path) => Ok(Box::new(FsStorage::new(path))),
        DeviceRoot::Mtp { device, storage } => mtp::system().open(device, *storage),
    }
}

/// A mounted volume.
pub struct FsStorage {
    root: PathBuf,
}

impl FsStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The local path for a device path, refusing anything that could leave the root.
    fn resolve(&self, path: &str) -> io::Result<PathBuf> {
        let relative = Path::new(path);
        let escapes = path.is_empty()
            || path.contains('\\')
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)));
        if escapes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{path:?} is not a path below the device root"),
            ));
        }
        Ok(self.root.join(relative))
    }
}

impl Storage for FsStorage {
    fn list(&self, dir: &str) -> io::Result<Vec<StoredFile>> {
        let mut files = Vec::new();
        walk(&self.resolve(dir)?, dir, &mut files)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    fn read(&self, path: &str) -> io::Result<Option<Vec<u8>>> {
        match fs::read(self.resolve(path)?) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn write(&self, path: &str, data: &[u8]) -> io::Result<()> {
        let target = self.resolve(path)?;
        let temp = temp_beside(&target);
        write_new(&temp, data).and_then(|()| publish(&temp, &target))
    }

    fn put(
        &self,
        source: &Path,
        dest: &str,
        progress: &mut dyn FnMut(u64) -> ControlFlow<()>,
    ) -> io::Result<u64> {
        let target = self.resolve(dest)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = temp_beside(&target);
        copy_to(source, &temp, progress).and_then(|size| publish(&temp, &target).map(|()| size))
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        match fs::remove_file(self.resolve(path)?) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    fn remove_dir_if_empty(&self, dir: &str) {
        if let Ok(path) = self.resolve(dir) {
            // Fails, harmlessly, for any folder that still holds something.
            let _ = fs::remove_dir(path);
        }
    }

    fn free_bytes(&self) -> io::Result<u64> {
        fs4::available_space(&self.root)
    }
}

fn walk(dir: &Path, relative: &str, files: &mut Vec<StoredFile>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        // A name that is not valid UTF-8 cannot be one of ours, and symlinks are never followed.
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let child = format!("{relative}/{name}");
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&entry.path(), &child, files)?;
        } else if kind.is_file() {
            files.push(StoredFile {
                path: child,
                size: entry.metadata()?.len(),
            });
        }
    }
    Ok(())
}

/// `dir/.name.as-part`, beside the file it will become so the rename stays on one file system.
fn temp_beside(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}{TEMP_SUFFIX}"))
}

fn write_new(temp: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = temp.parent() {
        fs::create_dir_all(parent)?;
    }
    let result = File::create(temp).and_then(|mut file| {
        file.write_all(data)?;
        file.sync_all()
    });
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn copy_to(
    source: &Path,
    temp: &Path,
    progress: &mut dyn FnMut(u64) -> ControlFlow<()>,
) -> io::Result<u64> {
    let result = (|| {
        let mut input = File::open(source)?;
        let mut output = File::create(temp)?;
        let mut buffer = vec![0u8; 1 << 20];
        let mut total = 0u64;
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
            total += read as u64;
            if progress(total).is_break() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "copy cancelled"));
            }
        }
        // A removable drive may acknowledge writes it has only cached; the rename must not run ahead
        // of the data.
        output.sync_all()?;
        Ok(total)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

/// Moves a finished temporary file over its destination. `rename` replaces an existing file on
/// every platform Rust supports.
fn publish(temp: &Path, target: &Path) -> io::Result<()> {
    let result = fs::rename(temp, target);
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with_sources() -> (tempfile::TempDir, FsStorage, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        fs::write(&source, vec![7u8; 3_000_000]).unwrap();
        let root = dir.path().join("device");
        fs::create_dir(&root).unwrap();
        (dir, FsStorage::new(&root), source)
    }

    #[test]
    fn put_creates_folders_copies_and_leaves_no_temp() {
        let (dir, storage, source) = dir_with_sources();
        let mut seen = Vec::new();
        let size = storage
            .put(&source, "Music/A/B/song.flac", &mut |total| {
                seen.push(total);
                ControlFlow::Continue(())
            })
            .unwrap();
        assert_eq!(size, 3_000_000);
        assert_eq!(
            fs::read(dir.path().join("device/Music/A/B/song.flac")).unwrap(),
            fs::read(&source).unwrap()
        );
        assert_eq!(seen.last(), Some(&3_000_000));
        assert!(seen.len() >= 3);
        let leftovers: Vec<_> = storage.list("Music").unwrap();
        assert_eq!(
            leftovers,
            [StoredFile {
                path: "Music/A/B/song.flac".into(),
                size: 3_000_000
            }]
        );
    }

    #[test]
    fn a_cancelled_put_leaves_neither_the_destination_nor_a_temp_file() {
        let (_dir, storage, source) = dir_with_sources();
        let err = storage
            .put(&source, "Music/song.flac", &mut |_| ControlFlow::Break(()))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(storage.list("Music").unwrap().is_empty());
    }

    #[test]
    fn a_cancelled_replacement_keeps_the_old_file() {
        let (_dir, storage, source) = dir_with_sources();
        storage.write("Music/song.flac", b"old").unwrap();
        storage
            .put(&source, "Music/song.flac", &mut |_| ControlFlow::Break(()))
            .unwrap_err();
        assert_eq!(storage.read("Music/song.flac").unwrap().unwrap(), b"old");
    }

    #[test]
    fn write_replaces_atomically_and_read_reports_absence() {
        let (_dir, storage, _) = dir_with_sources();
        assert_eq!(storage.read(".antistreaming/manifest.json").unwrap(), None);
        storage
            .write(".antistreaming/manifest.json", b"one")
            .unwrap();
        storage
            .write(".antistreaming/manifest.json", b"two")
            .unwrap();
        assert_eq!(
            storage
                .read(".antistreaming/manifest.json")
                .unwrap()
                .unwrap(),
            b"two"
        );
        assert_eq!(storage.list(".antistreaming").unwrap().len(), 1);
    }

    #[test]
    fn remove_ignores_missing_files_and_prunes_only_empty_folders() {
        let (_dir, storage, _) = dir_with_sources();
        storage.write("Music/A/song.flac", b"x").unwrap();
        storage.remove("Music/A/missing.flac").unwrap();
        storage.remove_dir_if_empty("Music/A");
        assert_eq!(storage.list("Music").unwrap().len(), 1);
        storage.remove("Music/A/song.flac").unwrap();
        storage.remove_dir_if_empty("Music/A");
        assert!(!storage.resolve("Music/A").unwrap().exists());
        storage.remove_dir_if_empty("Music/never-existed");
    }

    #[test]
    fn paths_cannot_leave_the_root() {
        let (_dir, storage, source) = dir_with_sources();
        for bad in ["../escape", "/etc/passwd", "a/../../b", "", "a\\b", "./x"] {
            assert_eq!(
                storage.read(bad).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{bad:?}"
            );
            assert!(storage.write(bad, b"x").is_err(), "{bad:?}");
            assert!(
                storage
                    .put(&source, bad, &mut |_| ControlFlow::Continue(()))
                    .is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn listing_is_recursive_sorted_and_skips_symlinks() {
        let (dir, storage, _) = dir_with_sources();
        storage.write("Music/b/2.flac", b"22").unwrap();
        storage.write("Music/a/1.flac", b"1").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path(), dir.path().join("device/Music/loop")).unwrap();
        let _ = &dir;
        let listing = storage.list("Music").unwrap();
        assert_eq!(
            listing,
            [
                StoredFile {
                    path: "Music/a/1.flac".into(),
                    size: 1
                },
                StoredFile {
                    path: "Music/b/2.flac".into(),
                    size: 2
                },
            ]
        );
        assert!(storage.list("Nothing").unwrap().is_empty());
    }

    #[test]
    fn free_space_is_reported() {
        let (_dir, storage, _) = dir_with_sources();
        assert!(storage.free_bytes().unwrap() > 0);
    }
}
