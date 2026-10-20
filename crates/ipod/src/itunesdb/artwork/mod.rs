//! Album artwork: thumbnails stored in `iPod_Control/Artwork/F<format>_<n>.ithmb` and the
//! `ArtworkDB` that says which track's art is where. Follows libgpod's db-artwork-writer.c and
//! ithumb-writer.c.
//!
//! Where libgpod rewrites every thumbnail file on each save, this appends: new thumbnails go on the
//! end of the existing `.ithmb` files and new image records on the end of the existing ArtworkDB,
//! so the artwork of tracks this crate did not add is never touched. Appending is also safe to
//! abandon: the old ArtworkDB never refers past the old end of a file, so a failed sync only has to
//! truncate what it appended.

mod db;
mod formats;
mod thumb;

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use self::db::{ArtworkDb, ThumbRecord, encode_image};
pub(crate) use self::formats::{ArtworkFormat, from_sysinfo_extended as formats_in};
use crate::error::{Error, Result};
use crate::models::IpodModel;
use crate::sysinfo;
use crate::time::unix_to_mac;

/// libgpod rolls over to a new `.ithmb` file after 256 MB, which kept the iPod's interface fast.
const ITHMB_MAX_LEN: u64 = 256_000_000;
/// Image ids start at 0x64 (libgpod's `min_id`).
const FIRST_IMAGE_ID: u32 = 0x64;

/// What a device wants: the thumbnail formats and whether tracks of one album share an image.
pub(crate) struct Artwork {
    formats: Vec<ArtworkFormat>,
    sparse: bool,
}

impl Artwork {
    /// `None` for iPods without album art or whose formats this crate cannot write.
    pub(crate) fn for_device(mount: &Path, model: Option<&IpodModel>) -> Option<Self> {
        let info = sysinfo::read(&mount.join("iPod_Control").join("Device"));
        let formats = info.artwork_formats.or_else(|| {
            model
                .map(|model| formats::for_model(model.family, model.generation))
                .filter(|formats| !formats.is_empty())
        })?;
        let sparse = match model {
            Some(model) => formats::supports_sparse(
                info.supports_sparse_artwork,
                model.family,
                model.generation,
            ),
            None => info.supports_sparse_artwork,
        };
        Some(Self { formats, sparse })
    }

    /// Whether tracks of one album with one cover share a single image (sparse artwork). Other
    /// iPods want an image of their own for every track.
    pub(crate) fn shares_images(&self) -> bool {
        self.sparse
    }

    /// Bytes of thumbnails one image adds, for the space estimate.
    pub(crate) fn bytes_per_image(&self) -> u64 {
        self.formats
            .iter()
            .map(|format| u64::from(format.slot_len()))
            .sum()
    }
}

/// What a read-only look at an existing ArtworkDB found.
pub(crate) struct Check {
    pub(crate) images: u64,
    pub(crate) thumbnails: u64,
    /// Thumbnails that point outside their `.ithmb` file (or into one that is missing).
    pub(crate) missing: u64,
    /// Whether writing the database back with nothing added reproduces it byte for byte, which
    /// is what keeps other programs' artwork intact.
    pub(crate) identical: bool,
}

/// Parses an ArtworkDB, checks its thumbnails against the `.ithmb` files, and re-emits it.
pub(crate) fn check(mount: &Path, bytes: &[u8]) -> Result<Check> {
    let db = ArtworkDb::parse(bytes)?;
    let (mut images, mut thumbnails, mut missing) = (0, 0, 0);
    for image in db.images() {
        images += 1;
        for thumb in image.thumbs() {
            thumbnails += 1;
            let file = artwork_dir(mount).join(thumb.file_name.trim_start_matches(':'));
            let end = u64::from(thumb.offset) + u64::from(thumb.size);
            if fs::metadata(file).map_or(true, |meta| meta.len() < end) {
                missing += 1;
            }
        }
    }
    let identical = db.emit(&[], &HashSet::new(), 0) == bytes;
    Ok(Check {
        images,
        thumbnails,
        missing,
        identical,
    })
}

pub(crate) fn database_path(mount: &Path) -> PathBuf {
    artwork_dir(mount).join("ArtworkDB")
}

fn artwork_dir(mount: &Path) -> PathBuf {
    mount.join("iPod_Control").join("Artwork")
}

/// What a track's `mhit` must say about its artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) image_id: u32,
    /// Size of the cover as it came from the library.
    pub(crate) source_len: u32,
}

/// An image this app made, identified by what it was made from so later syncs can reuse it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnownImage {
    pub(crate) key: String,
    pub(crate) image_id: u32,
}

/// The outcome of [`Writer::finish`]: the thumbnails are already appended to the `.ithmb` files,
/// and `database` is the ArtworkDB that makes them visible, ready to be committed.
pub(crate) struct Written {
    pub(crate) database: Option<Vec<u8>>,
    /// Every image this app owns after the sync, to record in the bookkeeping file.
    pub(crate) images: Vec<KnownImage>,
    appended: Vec<Appended>,
}

impl Written {
    /// Undoes the appends of a sync that failed before its ArtworkDB was committed.
    pub(crate) fn roll_back(&self) {
        roll_back(&self.appended);
    }
}

struct Appended {
    path: PathBuf,
    /// The file's length before, or `None` if it did not exist.
    original_len: Option<u64>,
}

fn roll_back(appended: &[Appended]) {
    for file in appended.iter().rev() {
        let _ = match file.original_len {
            None => fs::remove_file(&file.path),
            Some(len) => OpenOptions::new()
                .write(true)
                .open(&file.path)
                .and_then(|f| f.set_len(len)),
        };
    }
}

fn key_of(album: &str, data: &[u8], salt: Option<u64>) -> String {
    let mut sha = Sha1::new();
    sha.update(album.as_bytes());
    sha.update([0]);
    if let Some(salt) = salt {
        sha.update(salt.to_le_bytes());
    }
    sha.update(data);
    sha.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

enum Existing {
    Absent,
    Parsed(ArtworkDb),
    /// Left alone; it only matters if there turns out to be art to add.
    Damaged(Error),
}

/// Appends to one format's `.ithmb` files, rolling over to the next file after 256 MB.
struct Stream {
    format: ArtworkFormat,
    dir: PathBuf,
    index: u32,
    open: Option<(File, u64)>,
}

impl Stream {
    fn path(&self, index: u32) -> PathBuf {
        self.dir
            .join(format!("F{}_{index}.ithmb", self.format.format_id))
    }

    /// Writes one thumbnail and returns the file it landed in and the offset. Every thumbnail of a
    /// format is the same size, so offsets stay multiples of it.
    fn place(&mut self, pixels: &[u8], appended: &mut Vec<Appended>) -> Result<(String, u32)> {
        let slot = u64::from(self.format.slot_len());
        loop {
            let path = self.path(self.index);
            let (file, length) = match &mut self.open {
                Some(open) => open,
                None => {
                    let original_len = existing_len(&path)?;
                    if original_len.unwrap_or(0) >= ITHMB_MAX_LEN {
                        self.index += 1;
                        continue;
                    }
                    if !original_len.unwrap_or(0).is_multiple_of(slot) {
                        return Err(Error::Conflict(format!(
                            "{} is not a whole number of {slot}-byte thumbnails; artwork was not written",
                            path.display()
                        )));
                    }
                    // Recorded before the first write so a failure part-way can still undo it.
                    appended.push(Appended {
                        path: path.clone(),
                        original_len,
                    });
                    let file = OpenOptions::new()
                        .append(true)
                        .create(true)
                        .open(&path)
                        .map_err(|source| Error::io(&path, source))?;
                    self.open.insert((file, original_len.unwrap_or(0)))
                }
            };
            if *length >= ITHMB_MAX_LEN {
                self.close()?;
                self.index += 1;
                continue;
            }
            file.write_all(pixels)
                .map_err(|source| Error::io(&path, source))?;
            let offset = u32::try_from(*length).unwrap_or(u32::MAX);
            *length += slot;
            return Ok((
                format!(":F{}_{}.ithmb", self.format.format_id, self.index),
                offset,
            ));
        }
    }

    /// Flushes the current file to the device.
    fn close(&mut self) -> Result<()> {
        if let Some((file, _)) = self.open.take() {
            let path = self.path(self.index);
            file.sync_all().map_err(|source| Error::io(path, source))?;
        }
        Ok(())
    }
}

/// Renders covers one at a time and appends their thumbnails as it goes, so no more than one cover
/// and one image's thumbnails are in memory however large the library is.
///
/// On a device with sparse artwork, tracks of one album with the same cover share an image, and an
/// image made by an earlier sync (`known`) is reused if it is still in the ArtworkDB.
pub(crate) struct Writer<'a> {
    artwork: &'a Artwork,
    existing: Existing,
    in_db: HashSet<u32>,
    known: Vec<KnownImage>,
    reusable: HashMap<String, u32>,
    /// Images made in this run, by what they were made from.
    made: HashMap<String, Link>,
    reused: HashSet<u32>,
    streams: Vec<Stream>,
    new_images: Vec<Vec<u8>>,
    new_known: Vec<KnownImage>,
    next_id: u32,
    created_mac: u32,
    appended: Vec<Appended>,
}

impl<'a> Writer<'a> {
    pub(crate) fn open(
        artwork: &'a Artwork,
        mount: &Path,
        known: &[KnownImage],
        now_unix: i64,
    ) -> Result<Self> {
        let path = database_path(mount);
        let existing = match fs::read(&path) {
            Ok(bytes) => match ArtworkDb::parse(&bytes) {
                Ok(db) => Existing::Parsed(db),
                Err(error) => Existing::Damaged(error),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Existing::Absent,
            Err(source) => return Err(Error::io(path, source)),
        };
        let in_db = match &existing {
            Existing::Parsed(db) => db.image_ids(),
            Existing::Absent | Existing::Damaged(_) => HashSet::new(),
        };
        let reusable = known
            .iter()
            .filter(|image| in_db.contains(&image.image_id))
            .map(|image| (image.key.clone(), image.image_id))
            .collect();
        let dir = artwork_dir(mount);
        Ok(Self {
            artwork,
            streams: artwork
                .formats
                .iter()
                .map(|format| Stream {
                    format: format.clone(),
                    dir: dir.clone(),
                    index: 1,
                    open: None,
                })
                .collect(),
            next_id: in_db
                .iter()
                .copied()
                .max()
                .map_or(FIRST_IMAGE_ID, |max| max.max(FIRST_IMAGE_ID - 1) + 1),
            existing,
            in_db,
            known: known.to_vec(),
            reusable,
            made: HashMap::new(),
            reused: HashSet::new(),
            new_images: Vec::new(),
            new_known: Vec::new(),
            created_mac: unix_to_mac(now_unix),
            appended: Vec::new(),
        })
    }

    /// Gives the track `owner` (the first of its album to use this cover) an image for `data`, a
    /// JPEG or PNG. Returns `None` if the cover cannot be decoded: a bad cover must not stop a
    /// sync, so the track just gets no art.
    pub(crate) fn add(&mut self, owner: u64, album: &str, data: &[u8]) -> Result<Option<Link>> {
        let salt = (!self.artwork.sparse).then_some(owner);
        let key = key_of(album, data, salt);
        if let Some(&link) = self.made.get(&key) {
            return Ok(Some(link));
        }
        let source_len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        if self.artwork.sparse
            && let Some(&image_id) = self.reusable.get(&key)
        {
            self.reused.insert(image_id);
            let link = Link {
                image_id,
                source_len,
            };
            self.made.insert(key, link);
            return Ok(Some(link));
        }
        if let Existing::Damaged(error) = &self.existing {
            return Err(Error::corrupt(
                0,
                format!("the ArtworkDB cannot be read ({error})"),
            ));
        }
        let Some(source) = thumb::decode(data) else {
            return Ok(None);
        };

        let image_id = self.next_id;
        let mut records = Vec::with_capacity(self.streams.len());
        for stream in &mut self.streams {
            let thumb = thumb::render(&source, &stream.format);
            fs::create_dir_all(&stream.dir).map_err(|error| Error::io(&stream.dir, error))?;
            let (file_name, offset) = stream.place(&thumb.pixels, &mut self.appended)?;
            records.push(ThumbRecord {
                format_id: stream.format.format_id,
                file_name,
                offset,
                size: stream.format.slot_len(),
                width: thumb.width,
                height: thumb.height,
                horizontal_padding: thumb.horizontal_padding,
                vertical_padding: thumb.vertical_padding,
            });
        }
        self.next_id += 1;
        self.new_images.push(encode_image(
            image_id,
            owner,
            source_len,
            self.created_mac,
            &records,
        ));
        if self.artwork.sparse {
            self.new_known.push(KnownImage {
                key: key.clone(),
                image_id,
            });
        }
        let link = Link {
            image_id,
            source_len,
        };
        self.made.insert(key, link);
        Ok(Some(link))
    }

    /// Undoes what this writer appended; for a sync that fails before [`Writer::finish`] returns.
    pub(crate) fn roll_back(&self) {
        roll_back(&self.appended);
    }

    /// Flushes the thumbnails and builds the ArtworkDB. `referenced` holds the ids the on-disk
    /// iTunesDB still uses: images this app made that nothing uses any more are dropped, one sync
    /// late on purpose, so the database committed alongside never loses an image the previous
    /// iTunesDB still points at.
    pub(crate) fn finish(mut self, referenced: &HashSet<u32>) -> Result<Written> {
        let flushed: Result<()> = self.streams.iter_mut().try_for_each(Stream::close);
        if let Err(error) = flushed {
            self.roll_back();
            return Err(error);
        }
        let used: HashSet<u32> = self.made.values().map(|link| link.image_id).collect();
        let dropped: HashSet<u32> = self
            .known
            .iter()
            .map(|image| image.image_id)
            .filter(|id| !referenced.contains(id) && !used.contains(id))
            .collect();
        let removes_something = dropped.iter().any(|id| self.in_db.contains(id));
        let database = match (
            &self.existing,
            self.new_images.is_empty() && !removes_something,
        ) {
            (_, true) | (Existing::Damaged(_), false) => None,
            (Existing::Parsed(db), false) => {
                Some(db.emit(&self.new_images, &dropped, self.next_id))
            }
            (Existing::Absent, false) => Some(ArtworkDb::fresh(&self.artwork.formats).emit(
                &self.new_images,
                &dropped,
                self.next_id,
            )),
        };
        let images = self
            .known
            .iter()
            .filter(|image| !dropped.contains(&image.image_id))
            .cloned()
            .chain(self.new_known.drain(..))
            .collect();
        Ok(Written {
            database,
            images,
            appended: std::mem::take(&mut self.appended),
        })
    }
}

fn existing_len(path: &Path) -> Result<Option<u64>> {
    match fs::metadata(path) {
        Ok(meta) => Ok(Some(meta.len())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::io(path, source)),
    }
}
