use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use crate::error::{Error, Result};
use crate::files::{is_artwork, is_audio, is_junk};
use crate::ledger::{Inbox, Kind};
use crate::model::Purchase;
use crate::scan::survey;

/// Albums are a few hundred megabytes, and a 24-bit/192 kHz box set a few gigabytes. This is far
/// above that and far below a disk-filling zip bomb.
const MAX_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;

const FOLDER_NAME_LIMIT: usize = 120;

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub max_bytes: u64,
    pub max_entries: usize,
}

impl Limits {
    const DEFAULT: Self = Self {
        max_bytes: MAX_BYTES,
        max_entries: MAX_ENTRIES,
    };
}

impl Inbox {
    /// Copies an offered download into `<library>/<Album Artist>/<Album>/` and records it as a
    /// purchase. Nothing appears in the library unless all of it was written, and the zip or
    /// folder it came from is left alone. `progress` receives bytes written so far and the total.
    pub fn accept(
        &mut self,
        candidate_id: i64,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Purchase> {
        let source = self.pending(candidate_id)?;
        fs::create_dir_all(&self.library_dir)?;
        // Inside the library so the final rename stays on one filesystem. Dropping it on any
        // error path removes whatever was half-written.
        let staging = tempfile::Builder::new()
            .prefix(".antistreaming-import-")
            .tempdir_in(&self.library_dir)?;
        let written = match source.kind {
            Kind::Zip => extract_zip(&source.path, staging.path(), Limits::DEFAULT, progress)?,
            Kind::Folder => copy_folder(&source.path, staging.path(), Limits::DEFAULT, progress)?,
        };
        let mut wanted: Vec<&PathBuf> = written
            .iter()
            .filter(|path| {
                let name = path.to_string_lossy();
                is_audio(&name) || is_artwork(&name)
            })
            .collect();
        if !wanted.iter().any(|path| is_audio(&path.to_string_lossy())) {
            return Err(Error::NoAudio(source.path));
        }
        wanted.sort();

        let destination = place(
            &self.library_dir,
            staging.path(),
            &source.artist,
            &source.album,
        )?;
        let files: Vec<PathBuf> = wanted.iter().map(|path| destination.join(path)).collect();
        self.record_import(&source, &files).inspect_err(|_| {
            let _ = fs::remove_dir_all(&destination);
        })
    }
}

/// Moves the finished staging folder to its final name with one rename. An existing album
/// folder is never touched: the new one becomes "Album (2)".
fn place(library_dir: &Path, staging: &Path, artist: &str, album: &str) -> Result<PathBuf> {
    let artist_dir = library_dir.join(folder_name(artist, "Unknown Artist"));
    let created_artist_dir = !artist_dir.exists();
    fs::create_dir_all(&artist_dir)?;

    let album_name = folder_name(album, "Unknown Album");
    let mut destination = artist_dir.join(&album_name);
    let mut copy = 2;
    while destination.exists() {
        destination = artist_dir.join(format!("{album_name} ({copy})"));
        copy += 1;
    }
    if let Err(error) = fs::rename(staging, &destination) {
        if created_artist_dir {
            // Fails harmlessly if anything else has since been put there.
            let _ = fs::remove_dir(&artist_dir);
        }
        return Err(error.into());
    }
    Ok(destination)
}

/// Returns the paths written, relative to `staging`.
fn extract_zip(
    zip_path: &Path,
    staging: &Path,
    limits: Limits,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Vec<PathBuf>> {
    let mut archive = ZipArchive::new(BufReader::new(File::open(zip_path)?))?;
    if archive.len() > limits.max_entries {
        return Err(Error::TooManyEntries {
            limit: limits.max_entries,
        });
    }

    // Every name is checked before the first byte is written, so a hostile archive is refused
    // outright instead of being extracted up to the bad entry.
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index)?;
        let name = entry.name()?;
        let parts = safe_components(&name).ok_or_else(|| Error::UnsafeArchive(name.to_string()))?;
        if !entry.is_dir() && !parts.is_empty() && !is_junk(&name) {
            files.push((index, parts, entry.size()));
        }
    }
    let total: u64 = files.iter().map(|(_, _, size)| size).sum();
    if total > limits.max_bytes {
        return Err(Error::TooLarge {
            limit: limits.max_bytes,
        });
    }
    // Stores wrap the tracks in a folder named after the album; the library adds its own.
    if has_common_root(&files) {
        for (_, parts, _) in &mut files {
            parts.remove(0);
        }
    }

    let mut writer = Writer::new(staging, total, limits.max_bytes, progress);
    for (index, parts, _) in files {
        writer.write(&parts, &mut archive.by_index(index)?)?;
    }
    Ok(writer.written)
}

fn copy_folder(
    folder: &Path,
    staging: &Path,
    limits: Limits,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Vec<PathBuf>> {
    let survey = survey(folder);
    if survey.files.len() > limits.max_entries {
        return Err(Error::TooManyEntries {
            limit: limits.max_entries,
        });
    }
    let total = survey.bytes();
    if total > limits.max_bytes {
        return Err(Error::TooLarge {
            limit: limits.max_bytes,
        });
    }
    let mut writer = Writer::new(staging, total, limits.max_bytes, progress);
    for (relative, _) in &survey.files {
        let parts: Vec<String> = relative.split('/').map(sanitize).collect();
        writer.write(&parts, &mut File::open(folder.join(relative))?)?;
    }
    Ok(writer.written)
}

/// Writes files under the staging folder, counting the bytes that really arrive rather than
/// trusting sizes an archive declares.
struct Writer<'a> {
    staging: &'a Path,
    total: u64,
    limit: u64,
    done: u64,
    progress: &'a mut dyn FnMut(u64, u64),
    written: Vec<PathBuf>,
}

impl<'a> Writer<'a> {
    fn new(
        staging: &'a Path,
        total: u64,
        limit: u64,
        progress: &'a mut dyn FnMut(u64, u64),
    ) -> Self {
        progress(0, total);
        Self {
            staging,
            total,
            limit,
            done: 0,
            progress,
            written: Vec::new(),
        }
    }

    fn write(&mut self, parts: &[String], reader: &mut dyn Read) -> Result<()> {
        let Some((name, folders)) = parts.split_last() else {
            return Ok(());
        };
        let folder: PathBuf = folders.iter().collect();
        fs::create_dir_all(self.staging.join(&folder))?;
        let (mut file, name) = create_unique(&self.staging.join(&folder), name)?;

        let mut buffer = vec![0; 256 * 1024];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            self.done += read as u64;
            if self.done > self.limit {
                return Err(Error::TooLarge { limit: self.limit });
            }
            file.write_all(&buffer[..read])?;
            (self.progress)(self.done, self.total.max(self.done));
        }
        self.written.push(folder.join(name));
        Ok(())
    }
}

/// Opens a new file, never replacing one: a taken name becomes "name (2).ext", "name (3).ext".
fn create_unique(folder: &Path, name: &str) -> Result<(File, String)> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    let mut candidate = name.to_owned();
    let mut copy = 2;
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(folder.join(&candidate))
        {
            Ok(file) => return Ok((file, candidate)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                candidate = format!("{stem} ({copy}){extension}");
                copy += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Splits an archive entry name into plain path components, or `None` if extracting it could
/// land outside the destination: parent references, absolute paths, drive letters.
fn safe_components(name: &str) -> Option<Vec<String>> {
    if name.contains('\0') || name.starts_with(['/', '\\']) {
        return None;
    }
    let mut parts = Vec::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            _ => parts.push(part),
        }
    }
    let drive_letter = parts.first().is_some_and(|first| {
        let bytes = first.as_bytes();
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
    });
    if drive_letter {
        return None;
    }
    Some(parts.into_iter().map(sanitize).collect())
}

/// Whether every file sits inside the same single top folder.
fn has_common_root(files: &[(usize, Vec<String>, u64)]) -> bool {
    let Some((_, first, _)) = files.first() else {
        return false;
    };
    files
        .iter()
        .all(|(_, parts, _)| parts.len() > 1 && parts[0] == first[0])
}

/// Replaces what some filesystems refuse in a name, so one odd track title cannot fail an
/// extraction on Windows.
fn sanitize(part: &str) -> String {
    part.chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// An artist or album name made safe to use as a single folder name.
fn folder_name(name: &str, fallback: &str) -> String {
    let cleaned = sanitize(&name.replace(['/', '\\'], "_"));
    let cleaned: String = cleaned.chars().take(FOLDER_NAME_LIMIT).collect();
    // A leading dot would hide the folder and a trailing one or space is refused on Windows.
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::*;

    #[test]
    fn accepts_ordinary_entry_names() {
        assert_eq!(
            safe_components("Album/CD 1/01 Song.flac"),
            Some(vec!["Album".into(), "CD 1".into(), "01 Song.flac".into()])
        );
        assert_eq!(
            safe_components("./01 Song.flac"),
            Some(vec!["01 Song.flac".into()])
        );
        assert_eq!(safe_components("Album/"), Some(vec!["Album".into()]));
    }

    #[test]
    fn rejects_names_that_escape_the_destination() {
        for name in [
            "../evil.flac",
            "Album/../../evil.flac",
            "Album/..\\evil.flac",
            "/etc/passwd",
            "\\windows\\system32\\evil.dll",
            "C:/Windows/evil.dll",
            "C:evil.dll",
            "Album/\0.flac",
        ] {
            assert_eq!(safe_components(name), None, "{name:?}");
        }
    }

    #[test]
    fn keeps_names_that_merely_contain_dots() {
        assert!(safe_components("Album/Track... (live).flac").is_some());
        assert!(safe_components("..hidden/01.flac").is_some());
    }

    #[test]
    fn replaces_characters_windows_refuses() {
        assert_eq!(
            safe_components("What?/Who: Me*.flac"),
            Some(vec!["What_".into(), "Who_ Me_.flac".into()])
        );
    }

    #[test]
    fn folder_names_are_single_safe_components() {
        assert_eq!(folder_name("AC/DC", "x"), "AC_DC");
        assert_eq!(folder_name("..", "Unknown Artist"), "Unknown Artist");
        assert_eq!(folder_name("  ", "Unknown Album"), "Unknown Album");
        assert_eq!(folder_name("Is This It?", "x"), "Is This It_");
        assert_eq!(folder_name("Trailing dot.", "x"), "Trailing dot");
        assert_eq!(
            folder_name(&"a".repeat(300), "x").chars().count(),
            FOLDER_NAME_LIMIT
        );
    }

    #[test]
    fn taken_file_names_get_a_number_before_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (0..3)
            .map(|_| create_unique(dir.path(), "01 Song.flac").unwrap().1)
            .collect();
        assert_eq!(
            names,
            ["01 Song.flac", "01 Song (2).flac", "01 Song (3).flac"]
        );
        let bare: Vec<String> = (0..2)
            .map(|_| create_unique(dir.path(), "cover").unwrap().1)
            .collect();
        assert_eq!(bare, ["cover", "cover (2)"]);
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = ZipWriter::new(File::create(path).unwrap());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    fn extract(
        entries: &[(&str, &[u8])],
        limits: Limits,
    ) -> (tempfile::TempDir, std::result::Result<Vec<PathBuf>, Error>) {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("download.zip");
        write_zip(&zip, entries);
        let staging = dir.path().join("staging");
        fs::create_dir(&staging).unwrap();
        let result = extract_zip(&zip, &staging, limits, &mut |_, _| {});
        (dir, result)
    }

    const ROOMY: Limits = Limits {
        max_bytes: 1024,
        max_entries: 10,
    };

    #[test]
    fn declared_sizes_over_the_byte_cap_are_refused_before_writing() {
        let (dir, result) = extract(
            &[("01.flac", &[0; 600]), ("02.flac", &[0; 600])],
            Limits {
                max_bytes: 1000,
                ..ROOMY
            },
        );
        assert!(matches!(result, Err(Error::TooLarge { limit: 1000 })));
        assert_eq!(fs::read_dir(dir.path().join("staging")).unwrap().count(), 0);
    }

    #[test]
    fn too_many_entries_are_refused() {
        let entries: Vec<(String, [u8; 1])> =
            (0..11).map(|n| (format!("{n:02}.flac"), [0])).collect();
        let borrowed: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect();
        let (_dir, result) = extract(&borrowed, ROOMY);
        assert!(matches!(result, Err(Error::TooManyEntries { limit: 10 })));
    }

    #[test]
    fn the_writer_stops_at_the_cap_whatever_total_was_declared() {
        // What an archive says about its own sizes is not trusted: only bytes read count.
        let dir = tempfile::tempdir().unwrap();
        let mut progress = |_: u64, _: u64| {};
        let mut writer = Writer::new(dir.path(), 10, 100, &mut progress);
        let result = writer.write(&["bomb.bin".into()], &mut std::io::repeat(7));
        assert!(matches!(result, Err(Error::TooLarge { limit: 100 })));
        assert!(fs::metadata(dir.path().join("bomb.bin")).unwrap().len() <= 100);
    }

    #[test]
    fn a_single_top_folder_is_flattened() {
        let (_dir, result) = extract(
            &[("Album/01.flac", b"a"), ("Album/CD 2/02.flac", b"b")],
            ROOMY,
        );
        let mut written = result.unwrap();
        written.sort();
        assert_eq!(
            written,
            [PathBuf::from("01.flac"), PathBuf::from("CD 2/02.flac")]
        );
    }

    #[test]
    fn entries_in_different_folders_keep_their_layout() {
        let (_dir, result) = extract(&[("CD 1/01.flac", b"a"), ("CD 2/01.flac", b"b")], ROOMY);
        let mut written = result.unwrap();
        written.sort();
        assert_eq!(
            written,
            [PathBuf::from("CD 1/01.flac"), PathBuf::from("CD 2/01.flac")]
        );
    }

    #[test]
    fn names_that_collide_once_cleaned_are_kept_under_new_names() {
        // Zip itself refuses two entries with the same name; cleaning "?" to "_" can still make
        // two different names equal.
        let (dir, result) = extract(
            &[("01 What?.flac", b"first"), ("01 What_.flac", b"second")],
            ROOMY,
        );
        assert_eq!(
            result.unwrap(),
            [
                PathBuf::from("01 What_.flac"),
                PathBuf::from("01 What_ (2).flac")
            ]
        );
        let staging = dir.path().join("staging");
        assert_eq!(fs::read(staging.join("01 What_.flac")).unwrap(), b"first");
        assert_eq!(
            fs::read(staging.join("01 What_ (2).flac")).unwrap(),
            b"second"
        );
    }
}
