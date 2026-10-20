//! Housekeeping for the folder of downloaded songs. Every file this module manages is named
//! `<key>.<ext>`, or `<key>.<ext>.part` while it is still arriving, where the key is 32 hex
//! digits, so it never touches anything else that happens to be in the folder. The extension is
//! in the partial name too, so a download finished after a crash keeps its real one.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use md5::{Digest, Md5};

use crate::error::Result;

const PART: &str = "part";
const KEY_LEN: usize = 32;
/// A partial file untouched for this long was abandoned. A younger one may be a download
/// running right now in another thread, so it is left alone.
const ABANDONED_AFTER: Duration = Duration::from_secs(120);

/// Names a song on one server for one user. Hashed so that no character of an id can reach the
/// filesystem; MD5 is only used to spread names, not to protect anything.
pub(crate) fn key(base_url: &str, user: &str, song_id: &str) -> String {
    hex::encode(Md5::digest(format!("{base_url}\n{user}\n{song_id}")))
}

pub(crate) fn part_path(dir: &Path, key: &str, extension: &str) -> PathBuf {
    dir.join(format!("{key}.{extension}.{PART}"))
}

pub(crate) fn final_path(dir: &Path, key: &str, extension: &str) -> PathBuf {
    dir.join(format!("{key}.{extension}"))
}

/// The unfinished download of this song and the extension it was started with, if any.
pub(crate) fn find_partial(dir: &Path, key: &str) -> Option<(PathBuf, String)> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .find_map(|path| {
            let (found, is_part, extension) = parse(&path)?;
            // A partial file from before the extension went into its name has none to offer.
            let extension = if extension.is_empty() {
                "audio"
            } else {
                extension
            };
            let extension = extension.to_owned();
            (found == key && is_part).then_some((path, extension))
        })
}

/// The completed download of this song, if the cache has one.
pub(crate) fn find_finished(dir: &Path, key: &str) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .find(|path| parse(path).is_some_and(|(found, is_part, _)| found == key && !is_part))
}

/// Marks a file as just used, so eviction removes the least recently used songs first.
pub(crate) fn touch(path: &Path) {
    // Failing to update a timestamp only makes the file look older than it is.
    if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// Deletes the least recently used files of other songs until `incoming` more bytes fit under
/// `limit`. If the rest cannot be freed (they are downloads in progress), the download goes
/// ahead anyway: the cap is best-effort between songs and exact on each one.
pub(crate) fn make_room(dir: &Path, keep: &str, incoming: u64, limit: u64) -> Result<()> {
    let mut others: Vec<(PathBuf, u64, SystemTime, bool)> = Vec::new();
    for entry in fs::read_dir(dir)?.filter_map(std::result::Result::ok) {
        let path = entry.path();
        let Some((key, is_part, _)) = parse(&path) else {
            continue;
        };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if key != keep && metadata.is_file() {
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            others.push((path, metadata.len(), modified, is_part));
        }
    }
    let mut total = incoming + others.iter().map(|(_, size, _, _)| size).sum::<u64>();
    others.sort_by_key(|(_, _, modified, _)| *modified);
    for (path, size, modified, is_part) in others {
        if total <= limit {
            break;
        }
        let in_progress = is_part
            && SystemTime::now()
                .duration_since(modified)
                .is_ok_and(|age| age < ABANDONED_AFTER);
        if !in_progress && fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
    Ok(())
}

/// The key, whether it is a partial file, and the song's extension, for files this module owns.
/// A bare `<key>.part` (the older naming) has no extension.
fn parse(path: &Path) -> Option<(&str, bool, &str)> {
    let name = path.file_name()?.to_str()?;
    let (key, rest) = name.split_once('.')?;
    let is_key = key.len() == KEY_LEN && key.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !is_key {
        return None;
    }
    Some(match rest.strip_suffix(".part") {
        Some(extension) => (key, true, extension),
        None if rest == PART => (key, true, ""),
        None => (key, false, rest),
    })
}

/// What to call the file, so that a decoder can guess the format from the name.
pub(crate) fn extension(content_type: Option<&str>) -> &'static str {
    let essence = content_type
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase());
    match essence.as_deref() {
        Some("audio/flac" | "audio/x-flac") => "flac",
        Some("audio/mpeg" | "audio/mp3") => "mp3",
        Some("audio/mp4" | "audio/x-m4a" | "audio/m4a") => "m4a",
        Some("audio/aac") => "aac",
        Some("audio/ogg" | "application/ogg") => "ogg",
        Some("audio/opus") => "opus",
        Some("audio/wav" | "audio/x-wav" | "audio/wave") => "wav",
        Some("audio/aiff" | "audio/x-aiff") => "aiff",
        Some("audio/x-wavpack") => "wv",
        Some("audio/x-ms-wma") => "wma",
        _ => "audio",
    }
}

#[cfg(test)]
mod tests {
    use filetime::{FileTime, set_file_mtime};

    use super::*;

    fn key_of(n: u8) -> String {
        format!("{n:032x}")
    }

    fn write(dir: &Path, name: &str, size: usize, age_secs: u64) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, vec![0; size]).unwrap();
        let when = SystemTime::now() - Duration::from_secs(age_secs);
        set_file_mtime(&path, FileTime::from_system_time(when)).unwrap();
        path
    }

    #[test]
    fn keys_depend_on_server_user_and_song() {
        let key = key("http://a", "u", "1");
        assert_eq!(key.len(), 32);
        assert_ne!(key, super::key("http://b", "u", "1"));
        assert_ne!(key, super::key("http://a", "v", "1"));
        assert_ne!(key, super::key("http://a", "u", "2"));
        assert_eq!(key, super::key("http://a", "u", "1"));
    }

    #[test]
    fn ids_cannot_reach_the_filesystem() {
        let key = key("http://a", "u", "../../etc/passwd");
        assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn finds_a_finished_song_but_not_a_partial_one() {
        let dir = tempfile::tempdir().unwrap();
        let (done, partial) = (key_of(1), key_of(2));
        let song = write(dir.path(), &format!("{done}.flac"), 10, 0);
        write(dir.path(), &format!("{partial}.flac.part"), 10, 0);

        assert_eq!(find_finished(dir.path(), &done), Some(song));
        assert_eq!(find_finished(dir.path(), &partial), None);
        assert_eq!(find_finished(dir.path(), &key_of(3)), None);
        assert_eq!(find_finished(&dir.path().join("missing"), &done), None);
    }

    #[test]
    fn finds_a_partial_download_with_the_extension_it_started_with() {
        let dir = tempfile::tempdir().unwrap();
        let part = write(dir.path(), &format!("{}.flac.part", key_of(1)), 10, 0);
        write(dir.path(), &format!("{}.mp3", key_of(2)), 10, 0);

        assert_eq!(
            find_partial(dir.path(), &key_of(1)),
            Some((part, "flac".to_owned()))
        );
        assert_eq!(
            find_partial(dir.path(), &key_of(2)),
            None,
            "a finished song is not partial"
        );
        assert_eq!(find_partial(dir.path(), &key_of(3)), None);

        let legacy = write(dir.path(), &format!("{}.part", key_of(4)), 10, 0);
        assert_eq!(
            find_partial(dir.path(), &key_of(4)),
            Some((legacy, "audio".to_owned()))
        );
    }

    #[test]
    fn reads_the_parts_of_a_cache_file_name() {
        let key = key_of(1);
        let parsed = |name: String| {
            parse(Path::new(&name))
                .map(|(key, part, extension)| (key.to_owned(), part, extension.to_owned()))
        };
        assert_eq!(
            parsed(format!("{key}.flac.part")),
            Some((key.clone(), true, "flac".into()))
        );
        assert_eq!(
            parsed(format!("{key}.part")),
            Some((key.clone(), true, String::new()))
        );
        assert_eq!(
            parsed(format!("{key}.flac")),
            Some((key.clone(), false, "flac".into()))
        );
        assert_eq!(parsed("notes.txt".into()), None);
        assert_eq!(parsed(key.clone()), None);
    }

    #[test]
    fn evicts_the_least_recently_used_songs_first() {
        let dir = tempfile::tempdir().unwrap();
        let oldest = write(dir.path(), &format!("{}.mp3", key_of(1)), 100, 3000);
        let middle = write(dir.path(), &format!("{}.mp3", key_of(2)), 100, 2000);
        let newest = write(dir.path(), &format!("{}.mp3", key_of(3)), 100, 1000);

        // 300 held, 100 incoming, 250 allowed: two must go.
        make_room(dir.path(), &key_of(9), 100, 250).unwrap();

        assert!(!oldest.exists());
        assert!(!middle.exists());
        assert!(newest.exists());
    }

    #[test]
    fn leaves_everything_when_there_is_room() {
        let dir = tempfile::tempdir().unwrap();
        let song = write(dir.path(), &format!("{}.mp3", key_of(1)), 100, 10);

        make_room(dir.path(), &key_of(9), 100, 200).unwrap();

        assert!(song.exists());
    }

    #[test]
    fn never_evicts_the_song_being_fetched_nor_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let mine = write(dir.path(), &format!("{}.flac.part", key_of(1)), 500, 5000);
        let foreign = write(dir.path(), "notes.txt", 500, 5000);
        let nearly_hex = write(dir.path(), &format!("{}.mp3", "g".repeat(32)), 500, 5000);

        make_room(dir.path(), &key_of(1), 500, 10).unwrap();

        assert!(mine.exists() && foreign.exists() && nearly_hex.exists());
    }

    #[test]
    fn spares_partial_files_that_may_be_downloading_now() {
        let dir = tempfile::tempdir().unwrap();
        let active = write(dir.path(), &format!("{}.mp3.part", key_of(1)), 100, 5);
        let abandoned = write(dir.path(), &format!("{}.mp3.part", key_of(2)), 100, 3600);

        make_room(dir.path(), &key_of(9), 100, 100).unwrap();

        assert!(active.exists());
        assert!(!abandoned.exists());
    }

    #[test]
    fn touching_a_song_protects_it_from_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let loved = write(dir.path(), &format!("{}.mp3", key_of(1)), 100, 9000);
        let other = write(dir.path(), &format!("{}.mp3", key_of(2)), 100, 5000);

        touch(&loved);
        make_room(dir.path(), &key_of(9), 100, 200).unwrap();

        assert!(loved.exists());
        assert!(!other.exists());
    }

    #[test]
    fn names_files_by_content_type() {
        assert_eq!(extension(Some("audio/flac")), "flac");
        assert_eq!(extension(Some("Audio/MPEG; charset=binary")), "mp3");
        assert_eq!(extension(Some("audio/mp4")), "m4a");
        assert_eq!(extension(Some("application/octet-stream")), "audio");
        assert_eq!(extension(None), "audio");
    }
}
