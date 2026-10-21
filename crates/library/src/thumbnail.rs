use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;

use crate::artwork::{own_art, sibling_path};
use crate::error::Result;
use crate::library::Library;
use crate::model::{Artwork, MediaKind, Track};

const JPEG_QUALITY: u8 = 85;
/// Where, under the artwork cache, the thumbnails made from the covers kept for songs that are
/// only in iCloud go. They can be made again, unlike the covers.
const CLOUD_THUMBNAILS: &str = "Cloud";
/// Big enough for the largest grid, small enough that the covers of a whole library fit on a disk
/// that is nearly full.
const CLOUD_COVER_PIXELS: u32 = 512;

impl Library {
    pub fn set_artwork_cache(&mut self, dir: PathBuf) {
        self.artwork_cache = Some(dir);
    }

    pub fn set_cloud_covers(&mut self, dir: PathBuf) {
        self.cloud_covers = Some(dir);
    }

    /// A JPEG that fits within `size` x `size` (never enlarged), from the
    /// track's own art or else from another track of the same album, as iTunes
    /// shows an album's cover on every song. `artwork()` still returns the
    /// original, per track.
    pub fn artwork_thumbnail(&self, track_id: i64, size: u32) -> Result<Option<Artwork>> {
        let Some(track) = self.track(track_id)?.filter(|t| t.kind != MediaKind::Radio) else {
            return Ok(None);
        };
        let size = size.max(1);
        let cache = self.artwork_cache.as_deref();
        let dataless = &*self.is_dataless;
        if !track.in_cloud
            && let Some(art) =
                own_thumbnail(cache, Path::new(&track.path), &track.album, size, dataless)
        {
            return Ok(Some(art));
        }
        if (track.in_cloud || dataless(Path::new(&track.path)))
            && let Some(art) = self.cloud_thumbnail(&track, size)
        {
            return Ok(Some(art));
        }
        // Without an album name, "the same album" would pool unrelated songs.
        if track.album.is_empty() {
            return Ok(None);
        }

        let mut stmt = self.conn.prepare_cached(
            "SELECT path, album FROM tracks
             WHERE album_group = (SELECT album_group FROM tracks WHERE id = ?1) AND id <> ?1
               AND in_cloud = 0
             ORDER BY id",
        )?;
        let others = stmt.query_map([track_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for other in others {
            let (path, album) = other?;
            if let Some(art) = own_thumbnail(cache, Path::new(&path), &album, size, dataless) {
                return Ok(Some(art));
            }
        }
        Ok(None)
    }

    /// Keeps the cover of a song whose file is here, for when the song is back in the cloud and
    /// its file can no longer be read for it. Returns whether the song has a cover to keep.
    pub fn keep_cloud_cover(&self, track_id: i64) -> Result<bool> {
        let (Some(dir), Some(track)) = (self.cloud_covers.as_deref(), self.track(track_id)?) else {
            return Ok(false);
        };
        let Some(cover) = self
            .artwork(track_id)?
            .and_then(|art| shrink_to_jpeg(&art.bytes, CLOUD_COVER_PIXELS))
        else {
            return Ok(false);
        };
        Ok(keep_cover(dir, &track_cover_key(&track), &cover))
    }

    /// The cover `keep_cloud_cover` kept for a song that is in the cloud now.
    pub(crate) fn cloud_cover(&self, track: &Track) -> Option<Artwork> {
        let name = format!("{}.jpg", track_cover_key(track));
        Some(jpeg(fs::read(self.cloud_covers.as_ref()?.join(name)).ok()?))
    }

    fn cloud_thumbnail(&self, track: &Track, size: u32) -> Option<Artwork> {
        let name = format!("{}-{size}.jpg", track_cover_key(track));
        let cache = self
            .artwork_cache
            .as_ref()
            .map(|dir| dir.join(CLOUD_THUMBNAILS));
        if let Some(dir) = &cache
            && let Ok(bytes) = fs::read(dir.join(&name))
        {
            return Some(jpeg(bytes));
        }
        let cover = self.cloud_cover(track)?;
        // The kept cover is no bigger than this, and shrinking it to its own size would only
        // lose quality.
        if size >= CLOUD_COVER_PIXELS {
            return Some(cover);
        }
        let thumbnail = shrink_to_jpeg(&cover.bytes, size)?;
        if let Some(dir) = &cache {
            store(dir, &name, &thumbnail);
        }
        Some(jpeg(thumbnail))
    }

    /// Forgets the covers of songs that have left the library, which nothing else would: one is
    /// kept for every song read, for good. A picture that no song's cover links to goes with the
    /// last of them (where the file system can say so).
    pub fn tidy_cloud_covers(&self) -> Result<()> {
        let Some(dir) = self.cloud_covers.as_deref() else {
            return Ok(());
        };
        let mut songs = self
            .conn
            .prepare("SELECT path, file_size, date_modified FROM tracks")?;
        let wanted: HashSet<String> = songs
            .query_map([], |row| {
                Ok(cover_key(
                    &row.get::<_, String>(0)?,
                    row.get(1)?,
                    row.get(2)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let names = |extension: &str| -> Vec<fs::DirEntry> {
            fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|e| e == extension))
                .collect()
        };
        for entry in names("jpg") {
            let path = entry.path();
            let key = path.file_stem().and_then(|stem| stem.to_str());
            if key.is_some_and(|key| !wanted.contains(key)) {
                let _ = fs::remove_file(path);
            }
        }
        for entry in names("pic") {
            if only_link(&entry) {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn only_link(entry: &fs::DirEntry) -> bool {
    use std::os::unix::fs::MetadataExt;
    entry.metadata().is_ok_and(|metadata| metadata.nlink() == 1)
}

/// Windows cannot say how many names a file has without opening it; its pictures stay.
#[cfg(not(unix))]
fn only_link(_entry: &fs::DirEntry) -> bool {
    false
}

/// What a kept cover is filed under: the song's file as it was when read. A file that changes is
/// another song to the store, so a stale cover is never served.
fn cover_key(path: &str, file_size: u64, date_modified: i64) -> String {
    let mut hash = Fnv::new();
    hash.bytes(path.as_bytes());
    hash.bytes(&file_size.to_le_bytes());
    hash.bytes(&date_modified.to_le_bytes());
    format!("{:016x}", hash.0)
}

fn track_cover_key(track: &Track) -> String {
    cover_key(&track.path, track.file_size, track.date_modified)
}

/// The songs of an album mostly share one picture, and the disk may be nearly full, so each
/// distinct picture is stored once, under a name made from its contents, and a song's cover is a
/// link to it (or a copy, where the file system cannot link). A song's key names the file it was
/// read from as it was, so a cover already kept under it is the right one.
fn keep_cover(dir: &Path, key: &str, cover: &[u8]) -> bool {
    let name = format!("{key}.jpg");
    let entry = dir.join(&name);
    if entry.exists() {
        return true;
    }
    let mut hash = Fnv::new();
    hash.bytes(cover);
    let shared = format!("{:016x}.pic", hash.0);
    if !dir.join(&shared).exists() {
        store(dir, &shared, cover);
    }
    if fs::hard_link(dir.join(&shared), &entry).is_err() {
        store(dir, &name, cover);
    }
    entry.exists()
}

/// A missing file or undecodable picture simply means no art. Both outcomes
/// are cached, so a bare track costs a few `stat`s instead of a tag parse.
/// A file that is only in the cloud is neither read nor cached as bare: it
/// has art to give once it is local.
fn own_thumbnail(
    cache: Option<&Path>,
    track_path: &Path,
    album: &str,
    size: u32,
    dataless: &dyn Fn(&Path) -> bool,
) -> Option<Artwork> {
    if dataless(track_path) {
        return None;
    }
    let source = sibling_path(track_path, album, dataless);
    let key = format!(
        "{:016x}",
        source_key(track_path, source.as_ref().map(|(path, _)| path.as_path()))?
    );
    if let Some(dir) = cache {
        if let Ok(bytes) = fs::read(dir.join(format!("{key}-{size}.jpg"))) {
            return Some(jpeg(bytes));
        }
        if dir.join(format!("{key}.none")).exists() {
            return None;
        }
    }

    let thumbnail =
        own_art(track_path, album, dataless).and_then(|art| shrink_to_jpeg(&art.bytes, size));
    if let Some(dir) = cache {
        let (name, bytes) = match &thumbnail {
            Some(bytes) => (format!("{key}-{size}.jpg"), bytes.as_slice()),
            None => (format!("{key}.none"), &[][..]),
        };
        store(dir, &name, bytes);
    }
    thumbnail.map(jpeg)
}

fn jpeg(bytes: Vec<u8>) -> Artwork {
    Artwork {
        mime: "image/jpeg".to_owned(),
        bytes,
    }
}

/// A JPEG of the picture that fits within `size` x `size` (never enlarged), made in memory with
/// nothing cached. `None` if it cannot be decoded.
pub fn shrink_to_jpeg(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = if image.width().max(image.height()) > size {
        image.resize(size, size, FilterType::Triangle)
    } else {
        image
    };
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&image.to_rgb8())
        .ok()?;
    Some(out)
}

/// Identifies the inputs that decide the picture: the track file, and the
/// image beside it that would stand in. A change to either (or a new image
/// appearing) yields a new key, so stale thumbnails are never served.
fn source_key(track: &Path, sibling: Option<&Path>) -> Option<u64> {
    let mut hash = Fnv::new();
    hash.file(track)?;
    match sibling {
        Some(path) => hash.file(path)?,
        None => hash.bytes(b"-"),
    }
    Some(hash.0)
}

/// FNV-1a: unlike `DefaultHasher`, its output is specified, so keys stay valid
/// across builds.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn file(&mut self, path: &Path) -> Option<()> {
        let metadata = fs::metadata(path).ok()?;
        let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        self.bytes(path.as_os_str().as_encoded_bytes());
        self.bytes(&metadata.len().to_le_bytes());
        self.bytes(&modified.as_nanos().to_le_bytes());
        Some(())
    }
}

/// Written under a temporary name first so a concurrent reader never sees half
/// an image. Failing to cache costs speed, not correctness, so errors are dropped.
fn store(dir: &Path, name: &str, bytes: &[u8]) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = dir.join(format!(
        "{name}.{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = fs::create_dir_all(dir)
        .and_then(|()| fs::write(&temporary, bytes))
        .and_then(|()| fs::rename(&temporary, dir.join(name)));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_album_that_shares_one_picture_stores_it_once() {
        let dir = tempfile::TempDir::new().unwrap();
        let cover = vec![7u8; 100];

        assert!(keep_cover(dir.path(), "one", &cover));
        assert!(keep_cover(dir.path(), "two", &cover));
        // Kept again, as when a song is read a second time.
        assert!(keep_cover(dir.path(), "one", &cover));

        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names.len(), 3, "{names:?}");
        assert_eq!(
            names.iter().filter(|name| name.ends_with(".pic")).count(),
            1
        );
        assert_eq!(fs::read(dir.path().join("two.jpg")).unwrap(), cover);
    }
}
