use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, DirEntry, File, Metadata};
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use walkdir::WalkDir;
use zip::ZipArchive;

use crate::error::Result;
use crate::files::{extension, is_audio, is_in_progress, is_junk};
use crate::guess::{self, Guess};
use crate::ledger::{Inbox, Kind, Source, Status};
use crate::model::{Candidate, Store};
use crate::tags::{self, TagInfo};

/// A file written to this recently may still be growing. A download in progress always has a
/// fresh modification time, so this stands in for watching its size.
const SETTLE_TIME: Duration = Duration::from_secs(5);

/// How deep inside a folder download audio is looked for: disc folders and one more level.
pub(crate) const FOLDER_DEPTH: usize = 3;

/// Tags sit at the front of FLAC and most MP3s; reading more than this for them is wasted.
const TAG_READ_LIMIT: u64 = 32 * 1024 * 1024;

impl Inbox {
    /// Looks in each directory for finished downloads that have not been seen before: zip
    /// archives holding audio, and folders of audio that appeared since the directory was first
    /// scanned. Returns the ones found by this call; `candidates` lists everything pending.
    pub fn scan(&mut self, watch_dirs: &[PathBuf]) -> Result<Vec<Candidate>> {
        self.drop_vanished()?;
        let mut found = Vec::new();
        for dir in watch_dirs {
            let entries = match fs::read_dir(dir) {
                Ok(entries) => entries,
                // The Downloads folder of a user who has none is not a failure.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let first_scan = !self.is_watched(dir)?;
            let mut entries: Vec<DirEntry> = entries.filter_map(std::io::Result::ok).collect();
            entries.sort_by_key(DirEntry::file_name);
            // Safari unzips what it downloads, leaving a folder next to the zip it came from.
            let zip_stems: HashSet<String> = entries
                .iter()
                .filter_map(|entry| zip_stem(&entry.file_name().into_string().ok()?))
                .map(|stem| stem.to_lowercase())
                .collect();

            for entry in &entries {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if name.starts_with('.') || is_in_progress(&name) {
                    continue;
                }
                let path = entry.path();
                // Not following links keeps a symlink in Downloads from pulling in other places.
                let Ok(metadata) = fs::symlink_metadata(&path) else {
                    continue;
                };
                let candidate = if metadata.is_file() && zip_stem(&name).is_some() {
                    self.offer_zip(&path, &metadata)?
                } else if metadata.is_dir() && !zip_stems.contains(&name.to_lowercase()) {
                    self.offer_folder(&path, first_scan)?
                } else {
                    None
                };
                found.extend(candidate);
            }
            self.mark_watched(dir)?;
        }
        Ok(found)
    }

    fn offer_zip(&self, path: &Path, metadata: &Metadata) -> Result<Option<Candidate>> {
        let Ok(modified) = metadata.modified() else {
            return Ok(None);
        };
        if !settled(modified) {
            return Ok(None);
        }
        let (size, modified) = (metadata.len(), unix_millis(modified));
        if self.knows_file(path, size, modified)? {
            return Ok(None);
        }
        let Ok(file) = File::open(path) else {
            return Ok(None);
        };
        // Without its central directory, a zip is a download still being written. Nothing is
        // recorded so that the next scan tries again.
        let Ok(mut archive) = ZipArchive::new(BufReader::new(file)) else {
            return Ok(None);
        };

        let audio: Vec<String> = archive
            .file_names()
            // A name that cannot be decoded cannot be extracted either.
            .filter_map(std::result::Result::ok)
            .filter(|name| !name.ends_with('/') && is_audio(name) && !is_junk(name))
            .map(Cow::into_owned)
            .collect();
        if audio.is_empty() {
            self.insert_source(&Source::bare(
                path,
                Kind::Zip,
                size,
                modified,
                Status::Rejected,
            ))?;
            return Ok(None);
        }

        // Gone or locked since the metadata call: leave it for the next scan.
        let Ok(hash) = hash_file(path) else {
            return Ok(None);
        };
        if self.has_hash(&hash)? {
            let mut duplicate = Source::bare(path, Kind::Zip, size, modified, Status::Duplicate);
            duplicate.hash = Some(hash);
            self.insert_source(&duplicate)?;
            return Ok(None);
        }

        let stem = zip_stem(&file_name(path)).unwrap_or_default();
        let guess = guess::from_names(&stem, &audio);
        let tags = needs_tags(&guess)
            .then(|| zip_tags(&mut archive, &audio[0]))
            .flatten();
        let mut source = describe(path, Kind::Zip, &stem, guess, tags, &audio);
        source.size = size;
        source.modified = modified;
        source.hash = Some(hash);
        source.id = self.insert_source(&source)?;
        Ok(Some(source.candidate()))
    }

    fn offer_folder(&self, path: &Path, first_scan: bool) -> Result<Option<Candidate>> {
        let survey = survey(path);
        let audio = survey.audio();
        if audio.is_empty() || !survey.settled() {
            return Ok(None);
        }
        let hash = survey.fingerprint();
        if self.has_hash(&hash)? {
            return Ok(None);
        }

        let modified = unix_millis(survey.newest);
        if first_scan {
            let mut baseline = Source::bare(
                path,
                Kind::Folder,
                survey.bytes(),
                modified,
                Status::Baseline,
            );
            baseline.hash = Some(hash);
            self.insert_source(&baseline)?;
            return Ok(None);
        }

        let stem = file_name(path);
        let guess = guess::from_names(&stem, &audio);
        let tags = needs_tags(&guess)
            .then(|| File::open(path.join(&audio[0])).ok().and_then(tags::read))
            .flatten();
        let mut source = describe(path, Kind::Folder, &stem, guess, tags, &audio);
        source.size = survey.bytes();
        source.modified = modified;
        source.hash = Some(hash);
        source.id = self.insert_source(&source)?;
        Ok(Some(source.candidate()))
    }
}

/// What a walk of a folder download found.
pub(crate) struct Survey {
    /// Paths relative to the folder, `/`-separated, with sizes. Junk files are left out.
    pub files: Vec<(String, u64)>,
    /// The most recent modification time of any file or folder inside, folder itself included.
    pub newest: SystemTime,
    in_progress: bool,
}

pub(crate) fn survey(root: &Path) -> Survey {
    let mut survey = Survey {
        files: Vec::new(),
        newest: SystemTime::UNIX_EPOCH,
        in_progress: false,
    };
    let walk = WalkDir::new(root)
        .max_depth(FOLDER_DEPTH)
        .follow_links(false)
        .sort_by_file_name();
    for entry in walk.into_iter().filter_map(std::result::Result::ok) {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        // Directories count too: an archiver restores old times onto files it extracts, but the
        // folder it is filling still carries the time of the latest write.
        if let Ok(modified) = metadata.modified() {
            survey.newest = survey.newest.max(modified);
        }
        if !metadata.is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let relative = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if is_junk(&relative) {
            continue;
        }
        survey.in_progress |= is_in_progress(&relative);
        survey.files.push((relative, metadata.len()));
    }
    survey
}

impl Survey {
    pub(crate) fn audio(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|(path, _)| path.clone())
            .filter(|path| is_audio(path))
            .collect()
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.files.iter().map(|(_, size)| size).sum()
    }

    fn settled(&self) -> bool {
        !self.in_progress && settled(self.newest)
    }

    /// Names and sizes, not contents: hashing gigabytes of audio for a folder that is already on
    /// disk would be slow, and two folders with the same names and sizes are the same album.
    fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        for (path, size) in &self.files {
            hasher.update(path.as_bytes());
            hasher.update([0]);
            hasher.update(size.to_le_bytes());
        }
        format!("{:x}", hasher.finalize())
    }
}

/// Builds the ledger row for a download that is about to be offered. Names win over tags
/// because they are free to read; tags fill in whatever the names left unsaid.
fn describe(
    path: &Path,
    kind: Kind,
    stem: &str,
    guess: Guess,
    tags: Option<TagInfo>,
    audio: &[String],
) -> Source {
    let tags = tags.unwrap_or_default();
    let (fallback_artist, fallback_album) = guess::fallback_names(stem);
    let mut source = Source::bare(path, kind, 0, 0, Status::Pending);
    source.store = if guess.store == Store::Unknown {
        tags.store
    } else {
        guess.store
    };
    source.artist = guess.artist.or(tags.artist).unwrap_or(fallback_artist);
    source.album = guess.album.or(tags.album).unwrap_or(fallback_album);
    source.track_count = u32::try_from(audio.len()).unwrap_or(u32::MAX);
    source.formats = audio
        .iter()
        .filter_map(|path| extension(path))
        .map(|extension| extension.to_ascii_uppercase())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    source
}

fn needs_tags(guess: &Guess) -> bool {
    guess.store == Store::Unknown || guess.artist.is_none() || guess.album.is_none()
}

fn zip_tags<R: Read + Seek>(archive: &mut ZipArchive<R>, entry: &str) -> Option<TagInfo> {
    let mut data = Vec::new();
    archive
        .by_name(entry)
        .ok()?
        .take(TAG_READ_LIMIT)
        .read_to_end(&mut data)
        .ok()?;
    tags::read(Cursor::new(data))
}

fn settled(modified: SystemTime) -> bool {
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= SETTLE_TIME)
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 256 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(format!("{:x}", hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    }
}

fn zip_stem(name: &str) -> Option<String> {
    let (stem, extension) = name.rsplit_once('.')?;
    extension
        .eq_ignore_ascii_case("zip")
        .then(|| stem.to_owned())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn unix_millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}
