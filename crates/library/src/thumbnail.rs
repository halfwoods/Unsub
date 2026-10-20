use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;

use crate::artwork::{own_art, sibling_path};
use crate::error::Result;
use crate::library::Library;
use crate::model::{Artwork, MediaKind};

const JPEG_QUALITY: u8 = 85;

impl Library {
    pub fn set_artwork_cache(&mut self, dir: PathBuf) {
        self.artwork_cache = Some(dir);
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

    let thumbnail = own_art(track_path, album, dataless).and_then(|art| resize(&art.bytes, size));
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

fn resize(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
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
