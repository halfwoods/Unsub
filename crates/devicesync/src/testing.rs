//! Fixtures for the sync tests: a library of real source files, an in-memory device that can be
//! made small, and a device whose cable can be pulled.

use std::{
    collections::BTreeMap,
    fs, io,
    ops::ControlFlow,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use crate::{Storage, StoredFile, SyncItem};

/// Source files on the host, one per track.
pub(crate) struct Library {
    dir: tempfile::TempDir,
}

impl Library {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    /// A track whose file holds `2000 + id` bytes of a pattern unique to `id`.
    pub fn track(
        &self,
        id: i64,
        artist: &str,
        album: &str,
        title: &str,
        track: u32,
        ext: &str,
    ) -> SyncItem {
        self.track_of_size(
            id,
            artist,
            album,
            title,
            track,
            ext,
            2_000 + usize::try_from(id).unwrap(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn track_of_size(
        &self,
        id: i64,
        artist: &str,
        album: &str,
        title: &str,
        track: u32,
        ext: &str,
        size: usize,
    ) -> SyncItem {
        let source = self.dir.path().join(format!("{id}.{ext}"));
        fs::write(&source, vec![u8::try_from(id % 251).unwrap(); size]).unwrap();
        SyncItem {
            library_id: id,
            source,
            album_artist: artist.to_owned(),
            album: album.to_owned(),
            title: title.to_owned(),
            disc: Some(1),
            track: Some(track),
            size: size as u64,
            mtime: 1_700_000_000 + id,
        }
    }

    /// The same track after its source file changed.
    pub fn modified(&self, item: &SyncItem) -> SyncItem {
        let bytes = vec![0xEE; usize::try_from(item.size).unwrap() + 100];
        fs::write(&item.source, &bytes).unwrap();
        SyncItem {
            size: bytes.len() as u64,
            mtime: item.mtime + 1_000,
            ..item.clone()
        }
    }

    /// A file of `len` bytes outside the library, such as `prepare` would produce.
    pub fn scratch(&self, name: &str, len: usize) -> std::path::PathBuf {
        let path = self.dir.path().join(name);
        fs::write(&path, vec![0xAB; len]).unwrap();
        path
    }
}

/// A device held in memory, with a size.
pub(crate) struct MemoryStorage {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    capacity: u64,
}

impl MemoryStorage {
    pub fn new(capacity: u64) -> Self {
        Self {
            files: Mutex::default(),
            capacity,
        }
    }

    pub fn paths(&self) -> Vec<String> {
        self.files.lock().unwrap().keys().cloned().collect()
    }

    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(path).cloned()
    }

    fn used(files: &BTreeMap<String, Vec<u8>>) -> u64 {
        files.values().map(|bytes| bytes.len() as u64).sum()
    }

    fn store(&self, path: &str, bytes: Vec<u8>) -> io::Result<()> {
        let mut files = self.files.lock().unwrap();
        let others = Self::used(&files) - files.get(path).map_or(0, |old| old.len() as u64);
        if others + bytes.len() as u64 > self.capacity {
            return Err(io::Error::new(io::ErrorKind::StorageFull, "device is full"));
        }
        files.insert(path.to_owned(), bytes);
        Ok(())
    }
}

impl Storage for MemoryStorage {
    fn list(&self, dir: &str) -> io::Result<Vec<StoredFile>> {
        let prefix = format!("{dir}/");
        Ok(self
            .files
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path.starts_with(&prefix))
            .map(|(path, bytes)| StoredFile {
                path: path.clone(),
                size: bytes.len() as u64,
            })
            .collect())
    }

    fn read(&self, path: &str) -> io::Result<Option<Vec<u8>>> {
        Ok(self.get(path))
    }

    fn write(&self, path: &str, data: &[u8]) -> io::Result<()> {
        self.store(path, data.to_vec())
    }

    fn put(
        &self,
        source: &Path,
        dest: &str,
        progress: &mut dyn FnMut(u64) -> ControlFlow<()>,
    ) -> io::Result<u64> {
        let bytes = fs::read(source)?;
        let mut done = 0u64;
        for chunk in bytes.chunks(512) {
            done += chunk.len() as u64;
            if progress(done).is_break() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "copy cancelled"));
            }
        }
        let size = bytes.len() as u64;
        self.store(dest, bytes).map(|()| size)
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        self.files.lock().unwrap().remove(path);
        Ok(())
    }

    fn remove_dir_if_empty(&self, _: &str) {}

    fn free_bytes(&self) -> io::Result<u64> {
        Ok(self
            .capacity
            .saturating_sub(Self::used(&self.files.lock().unwrap())))
    }
}

impl Storage for Arc<MemoryStorage> {
    fn list(&self, dir: &str) -> io::Result<Vec<StoredFile>> {
        (**self).list(dir)
    }

    fn read(&self, path: &str) -> io::Result<Option<Vec<u8>>> {
        (**self).read(path)
    }

    fn write(&self, path: &str, data: &[u8]) -> io::Result<()> {
        (**self).write(path, data)
    }

    fn put(
        &self,
        source: &Path,
        dest: &str,
        progress: &mut dyn FnMut(u64) -> ControlFlow<()>,
    ) -> io::Result<u64> {
        (**self).put(source, dest, progress)
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        (**self).remove(path)
    }

    fn remove_dir_if_empty(&self, dir: &str) {
        (**self).remove_dir_if_empty(dir);
    }

    fn free_bytes(&self) -> io::Result<u64> {
        (**self).free_bytes()
    }
}

/// A device whose Nth copy fails. With `disconnect` it fails the way a pulled cable does: that copy
/// leaves a half-written temporary file, and nothing on the device can be reached afterwards. A new
/// `Faulty` (or the bare storage) is the cable plugged back in.
pub(crate) struct Faulty<'a> {
    inner: &'a dyn Storage,
    puts: AtomicUsize,
    fail_put: usize,
    disconnect: bool,
    down: AtomicBool,
}

impl<'a> Faulty<'a> {
    pub fn failing_put(inner: &'a dyn Storage, nth: usize, disconnect: bool) -> Self {
        Self {
            inner,
            puts: AtomicUsize::new(0),
            fail_put: nth,
            disconnect,
            down: AtomicBool::new(false),
        }
    }

    fn check(&self) -> io::Result<()> {
        if self.down.load(Ordering::SeqCst) {
            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "the device was unplugged",
            ))
        } else {
            Ok(())
        }
    }
}

impl Storage for Faulty<'_> {
    fn list(&self, dir: &str) -> io::Result<Vec<StoredFile>> {
        self.check()?;
        self.inner.list(dir)
    }

    fn read(&self, path: &str) -> io::Result<Option<Vec<u8>>> {
        self.check()?;
        self.inner.read(path)
    }

    fn write(&self, path: &str, data: &[u8]) -> io::Result<()> {
        self.check()?;
        self.inner.write(path, data)
    }

    fn put(
        &self,
        source: &Path,
        dest: &str,
        progress: &mut dyn FnMut(u64) -> ControlFlow<()>,
    ) -> io::Result<u64> {
        self.check()?;
        if self.puts.fetch_add(1, Ordering::SeqCst) + 1 != self.fail_put {
            return self.inner.put(source, dest, progress);
        }
        if self.disconnect {
            let (dir, name) = dest.rsplit_once('/').unwrap();
            self.inner
                .write(&format!("{dir}/.{name}.as-part"), b"half a song")?;
            self.down.store(true, Ordering::SeqCst);
        }
        Err(io::Error::other("the copy failed"))
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        self.check()?;
        self.inner.remove(path)
    }

    fn remove_dir_if_empty(&self, dir: &str) {
        if self.check().is_ok() {
            self.inner.remove_dir_if_empty(dir);
        }
    }

    fn free_bytes(&self) -> io::Result<u64> {
        self.check()?;
        self.inner.free_bytes()
    }
}
