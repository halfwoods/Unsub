use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::model::MediaKind;
use crate::normalize::trailing_group;
use crate::tags::TrackMeta;

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const READ_CHUNK: usize = 1 << 20;

/// Whether the file's contents are not on this machine. Asking costs one
/// `stat`, which never fetches them; opening the file would.
///
/// `SF_DATALESS` is the flag APFS sets on iCloud Drive placeholders. The block
/// count is no substitute: transparently compressed small files also report
/// none, so it would flag ordinary files.
#[cfg(target_os = "macos")]
pub(crate) fn platform_is_dataless(path: &Path) -> bool {
    use std::os::macos::fs::MetadataExt;
    const SF_DATALESS: u32 = 0x4000_0000;
    fs::metadata(path).is_ok_and(|metadata| metadata.st_flags() & SF_DATALESS != 0)
}

/// Windows marks the placeholders of its Cloud Files providers (OneDrive, iCloud for Windows)
/// with the recall attributes, and older offline-storage tools with `OFFLINE`. Asking for the
/// attributes opens the file without data access, which never fetches it; only reading does.
#[cfg(windows)]
pub(crate) fn platform_is_dataless(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    const NOT_HERE: u32 = FILE_ATTRIBUTE_OFFLINE
        | FILE_ATTRIBUTE_RECALL_ON_OPEN
        | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;
    fs::metadata(path).is_ok_and(|metadata| metadata.file_attributes() & NOT_HERE != 0)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn platform_is_dataless(_path: &Path) -> bool {
    false
}

/// Whether `path` is a cloud placeholder (iCloud Drive on macOS, a Cloud Files provider on
/// Windows) whose contents aren't on this machine yet. Never downloads; callers use it to fetch a
/// file before something blocks on opening it.
pub fn is_dataless(path: &Path) -> bool {
    platform_is_dataless(path)
}

/// What `ensure_local` needs from the operating system, so a test can stand in
/// for a cloud that cannot be conjured on demand.
trait Storage {
    fn is_dataless(&self, path: &Path) -> bool;
    /// Asks the system to fetch the file in the background.
    fn start_download(&self, path: &Path) -> io::Result<()>;
    /// Bytes already stored locally, for progress.
    fn bytes_on_disk(&self, path: &Path) -> u64;
}

struct System;

impl Storage for System {
    fn is_dataless(&self, path: &Path) -> bool {
        platform_is_dataless(path)
    }

    #[cfg(target_os = "macos")]
    fn start_download(&self, path: &Path) -> io::Result<()> {
        use std::process::{Command, Stdio};
        // The command-line face of iCloud Drive's "Download Now"; it returns as
        // soon as the download is requested.
        let status = Command::new("brctl")
            .arg("download")
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("brctl could not start the download"))
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn start_download(&self, _path: &Path) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    #[cfg(target_os = "macos")]
    fn bytes_on_disk(&self, path: &Path) -> u64 {
        use std::os::macos::fs::MetadataExt;
        fs::metadata(path).map_or(0, |metadata| metadata.st_blocks() * 512)
    }

    #[cfg(not(target_os = "macos"))]
    fn bytes_on_disk(&self, _path: &Path) -> u64 {
        0
    }
}

/// Makes sure the file's contents are on this machine, downloading them from
/// the cloud if need be, and reports `(bytes so far, total bytes)`. Returns at
/// once for a file that is already local, and on platforms without cloud
/// placeholders. Setting `cancel` stops the wait with `Error::Cancelled`; a
/// download already requested carries on in the background.
pub fn ensure_local(
    path: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<()> {
    wait_for_local(path, progress, cancel, &System, POLL_INTERVAL)
}

fn wait_for_local(
    path: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
    storage: &dyn Storage,
    poll: Duration,
) -> Result<()> {
    if !storage.is_dataless(path) {
        return Ok(());
    }
    let total = fs::metadata(path)?.len();
    progress(0, total);
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    if storage.start_download(path).is_err() {
        // Not an iCloud Drive item, no `brctl`, or not macOS: any read makes the
        // system fetch the file, and reading in pieces at least reports progress.
        read_through(path, total, progress, cancel)?;
        // The flag can outlive the fetch (a provider may keep the file marked, or
        // mark it for good), so waiting for it to clear could never end.
        progress(total, total);
        return Ok(());
    }
    // The fetch finishes on the system's schedule, so watch for the flag to clear.
    while storage.is_dataless(path) {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        progress(storage.bytes_on_disk(path).min(total), total);
        thread::sleep(poll);
    }
    progress(total, total);
    Ok(())
}

fn read_through(
    path: &Path,
    total: u64,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<()> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0; READ_CHUNK];
    let mut done = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        done += read as u64;
        progress(done.min(total), total);
    }
}

/// What the path alone suggests about a file that cannot be opened yet: the
/// title from the file name, minus any leading track number ("01 - ", "1-02 ",
/// "03. "), and artist and album from the two folders above it when it sits at
/// least two levels below the folder that was scanned (`Artist/Album/file`).
pub(crate) fn placeholder_meta(path: &Path, root: Option<&Path>) -> TrackMeta {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (disc_number, track_number, title) = match split_track_prefix(&stem) {
        Some((disc, track, title)) => (disc, Some(track), title),
        None => (None, None, stem),
    };
    let (artist, album) = root
        .and_then(|root| path.strip_prefix(root).ok())
        .and_then(|relative| {
            let folders: Vec<_> = relative.parent()?.components().collect();
            let [.., artist, album] = folders.as_slice() else {
                return None;
            };
            Some((
                artist.as_os_str().to_string_lossy().into_owned(),
                album.as_os_str().to_string_lossy().into_owned(),
            ))
        })
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    TrackMeta {
        title,
        artist,
        album_artist: String::new(),
        album,
        genre: String::new(),
        composer: String::new(),
        year: None,
        track_number,
        track_count: None,
        disc_number,
        disc_count: None,
        duration_ms: 0,
        bitrate_kbps: None,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        codec: codec_for(&extension),
        kind: if extension == "m4b" {
            MediaKind::Audiobook
        } else {
            MediaKind::Music
        },
        compilation: false,
        loudness: None,
        has_lyrics: false,
        sort_title: None,
        sort_artist: None,
        sort_album: None,
        sort_album_artist: None,
    }
}

/// Only the container is known, so ALAC in an `.m4a` reads as AAC until the
/// file is downloaded and parsed.
fn codec_for(extension: &str) -> &'static str {
    match extension {
        "mp3" => "MP3",
        "flac" => "FLAC",
        "ogg" | "oga" => "Vorbis",
        "opus" => "Opus",
        "wav" => "WAV",
        "aif" | "aiff" => "AIFF",
        "wv" => "WavPack",
        "ape" => "APE",
        _ => "AAC",
    }
}

/// Peels "disc-track" or "track" numbers off the front of a file name. A bare
/// number only counts when it is zero-padded ("07 Song") or followed by
/// punctuation ("7. Song"), so "7 Rings" and "2Pac" keep theirs. Names that are
/// numbers all the way down ("1-800-273-8255", "99.9F°"), a word after a dash
/// ("3-Way") and a title that is only a bracketed note ("007 (Shanty Town)") are
/// the song's own.
pub(crate) fn split_track_prefix(stem: &str) -> Option<(Option<u32>, u32, String)> {
    const SEPARATORS: [char; 7] = [' ', '.', '-', '_', ')', '–', '—'];
    let leading_digits = |text: &str| text.bytes().take_while(u8::is_ascii_digit).count().min(3);
    let first = leading_digits(stem);
    if first == 0 {
        return None;
    }
    let (mut disc, mut track, mut rest) = (None, &stem[..first], &stem[first..]);
    if let Some(after) = rest.strip_prefix('-') {
        let second = leading_digits(after);
        if second > 0 && !after[second..].starts_with(|c: char| c.is_ascii_digit()) {
            (disc, track, rest) = (Some(track), &after[..second], &after[second..]);
        }
    }
    let title = rest.trim_start_matches(SEPARATORS);
    let separator = &rest[..rest.len() - title.len()];
    let bare_dash = separator.chars().all(|c| matches!(c, '-' | '–' | '—'));
    let looks_numbered = disc.is_some()
        || track.starts_with('0')
        || (separator.contains(&SEPARATORS[1..]) && !bare_dash);
    let more_numbers = !separator.contains(' ') && title.starts_with(|c: char| c.is_ascii_digit());
    // Only a bare space: a dash after "03 - (Untitled)" says the number is a track's.
    let only_a_note =
        separator == " " && trailing_group(title).is_some_and(|(before, _)| before.is_empty());
    if title.is_empty() || separator.is_empty() || !looks_numbered || more_numbers || only_a_note {
        return None;
    }
    Some((
        disc.and_then(|d| d.parse().ok()),
        track.parse().ok()?,
        title.to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    use super::*;

    fn meta(path: &str, root: Option<&str>) -> TrackMeta {
        placeholder_meta(Path::new(path), root.map(Path::new))
    }

    #[test]
    fn names_are_split_into_track_number_and_title() {
        let cases: [(&str, Option<u32>, Option<u32>, &str); 29] = [
            ("01 - Halo", None, Some(1), "Halo"),
            ("02. Halo", None, Some(2), "Halo"),
            ("03 Halo", None, Some(3), "Halo"),
            ("04_Halo", None, Some(4), "Halo"),
            ("05) Halo", None, Some(5), "Halo"),
            ("1-02 Halo", Some(1), Some(2), "Halo"),
            ("2-11 - Halo", Some(2), Some(11), "Halo"),
            ("007 Bond Theme", None, Some(7), "Bond Theme"),
            ("7. Rings", None, Some(7), "Rings"),
            ("7 Rings", None, None, "7 Rings"),
            ("2Pac Intro", None, None, "2Pac Intro"),
            ("1999", None, None, "1999"),
            ("01", None, None, "01"),
            ("Halo", None, None, "Halo"),
            ("3 – Halo", None, Some(3), "Halo"),
            ("3 — Halo", None, Some(3), "Halo"),
            ("01–Halo", None, Some(1), "Halo"),
            (
                "01 (Sittin' On) The Dock of the Bay",
                None,
                Some(1),
                "(Sittin' On) The Dock of the Bay",
            ),
            ("007 (Shanty Town)", None, None, "007 (Shanty Town)"),
            ("03 - (Untitled)", None, Some(3), "(Untitled)"),
            ("12 - [Hidden Track]", None, Some(12), "[Hidden Track]"),
            (
                "3-Way (The Golden Rule)",
                None,
                None,
                "3-Way (The Golden Rule)",
            ),
            ("19-2000", None, None, "19-2000"),
            ("1-800-273-8255", None, None, "1-800-273-8255"),
            ("1-2-3", None, None, "1-2-3"),
            ("2-4-6-8 Motorway", None, None, "2-4-6-8 Motorway"),
            ("99.9F°", None, None, "99.9F°"),
            ("10 Years", None, None, "10 Years"),
            ("99 Luftballons", None, None, "99 Luftballons"),
        ];
        for (stem, disc, track, title) in cases {
            let found = meta(&format!("/m/{stem}.mp3"), None);
            assert_eq!(
                (found.disc_number, found.track_number, found.title.as_str()),
                (disc, track, title),
                "{stem}"
            );
        }
    }

    #[test]
    fn a_name_that_merely_starts_with_digits_keeps_them() {
        for name in [
            "867-5309/Jenny",
            "2 + 2 = 5",
            "1979",
            "99 Luftballons",
            "3-Way",
            "[1] Intro",
        ] {
            assert_eq!(split_track_prefix(name), None, "{name}");
        }
        assert_eq!(
            split_track_prefix("01 (Sittin' On) The Dock of the Bay"),
            Some((None, 1, "(Sittin' On) The Dock of the Bay".to_owned()))
        );
    }

    #[test]
    fn artist_and_album_come_from_the_two_folders_above_the_file() {
        let root = Some("/cloud/Music");
        let deep = meta("/cloud/Music/Beyonc\u{e9}/I Am/03 - Halo.mp3", root);
        assert_eq!(
            (deep.artist.as_str(), deep.album.as_str()),
            ("Beyonc\u{e9}", "I Am")
        );
        let deeper = meta("/cloud/Music/iTunes/Queen/Opera/Song.m4a", root);
        assert_eq!(
            (deeper.artist.as_str(), deeper.album.as_str()),
            ("Queen", "Opera")
        );
        for shallow in [
            meta("/cloud/Music/Album/Song.mp3", root),
            meta("/cloud/Music/Song.mp3", root),
            meta("/elsewhere/Artist/Album/Song.mp3", root),
            meta("/cloud/Music/Artist/Album/Song.mp3", None),
        ] {
            assert_eq!((shallow.artist.as_str(), shallow.album.as_str()), ("", ""));
        }
    }

    #[test]
    fn codec_and_kind_follow_the_extension() {
        let codecs = [
            ("a.MP3", "MP3"),
            ("a.flac", "FLAC"),
            ("a.ogg", "Vorbis"),
            ("a.opus", "Opus"),
            ("a.wav", "WAV"),
            ("a.aiff", "AIFF"),
            ("a.wv", "WavPack"),
            ("a.ape", "APE"),
            ("a.m4a", "AAC"),
            ("a.mp4", "AAC"),
        ];
        for (name, codec) in codecs {
            let found = meta(name, None);
            assert_eq!(
                (found.codec, found.kind),
                (codec, MediaKind::Music),
                "{name}"
            );
        }
        assert_eq!(meta("book.m4b", None).kind, MediaKind::Audiobook);
        assert_eq!(meta("a.mp3", None).duration_ms, 0);
    }

    /// A cloud that fetches a file after `fetched_after` checks of the flag. If
    /// `start_fails` it cannot be asked to, and the flag stays set even once the
    /// file has been read, as some providers leave it.
    struct FakeCloud {
        dataless_checks: Cell<u32>,
        fetched_after: u32,
        start_fails: bool,
        started: Cell<u32>,
        on_disk: RefCell<Vec<u64>>,
    }

    impl FakeCloud {
        fn new(fetched_after: u32, start_fails: bool) -> Self {
            Self {
                dataless_checks: Cell::new(0),
                fetched_after,
                start_fails,
                started: Cell::new(0),
                on_disk: RefCell::new(vec![10, 20, 40]),
            }
        }
    }

    impl Storage for FakeCloud {
        fn is_dataless(&self, _path: &Path) -> bool {
            let checks = self.dataless_checks.get();
            self.dataless_checks.set(checks + 1);
            self.start_fails || checks < self.fetched_after
        }

        fn start_download(&self, _path: &Path) -> io::Result<()> {
            self.started.set(self.started.get() + 1);
            if self.start_fails {
                Err(io::Error::other("no brctl"))
            } else {
                Ok(())
            }
        }

        fn bytes_on_disk(&self, _path: &Path) -> u64 {
            let mut sizes = self.on_disk.borrow_mut();
            if sizes.len() > 1 {
                sizes.remove(0)
            } else {
                sizes[0]
            }
        }
    }

    fn file_of(size: usize) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("song.mp3");
        fs::write(&path, vec![7u8; size]).unwrap();
        (dir, path)
    }

    fn run(cloud: &FakeCloud, path: &Path, cancel: &AtomicBool) -> (Result<()>, Vec<(u64, u64)>) {
        let mut seen = Vec::new();
        let result = wait_for_local(
            path,
            &mut |done, total| {
                seen.push((done, total));
                // A wait that never ends must fail the test, not hang it.
                if seen.len() > 1_000 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            cancel,
            cloud,
            Duration::from_millis(1),
        );
        (result, seen)
    }

    #[test]
    fn a_local_file_returns_at_once_without_asking_for_anything() {
        let (_dir, path) = file_of(100);
        let cloud = FakeCloud::new(0, false);

        let (result, seen) = run(&cloud, &path, &AtomicBool::new(false));

        result.unwrap();
        assert!(seen.is_empty());
        assert_eq!(cloud.started.get(), 0);
    }

    #[test]
    fn a_placeholder_is_fetched_and_progress_climbs_to_the_total() {
        let (_dir, path) = file_of(100);
        let cloud = FakeCloud::new(4, false);

        let (result, seen) = run(&cloud, &path, &AtomicBool::new(false));

        result.unwrap();
        assert_eq!(cloud.started.get(), 1);
        assert_eq!(seen.first(), Some(&(0, 100)));
        assert_eq!(seen.last(), Some(&(100, 100)));
        assert!(
            seen.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "{seen:?}"
        );
        assert!(
            seen.iter()
                .all(|(done, total)| done <= total && *total == 100)
        );
        assert!(
            seen.iter().any(|(done, _)| *done == 10 || *done == 20),
            "uses the bytes on disk: {seen:?}"
        );
    }

    #[test]
    fn without_a_way_to_start_the_download_a_complete_read_ends_the_wait() {
        let (_dir, path) = file_of(3 * READ_CHUNK + 5);
        let cloud = FakeCloud::new(0, true);
        let total = (3 * READ_CHUNK + 5) as u64;

        let (result, seen) = run(&cloud, &path, &AtomicBool::new(false));

        result.unwrap();
        assert_eq!(cloud.started.get(), 1);
        let reads: Vec<u64> = seen.iter().map(|(done, _)| *done).collect();
        assert!(
            reads.contains(&(READ_CHUNK as u64)) && reads.contains(&total),
            "{reads:?}"
        );
        assert_eq!(seen.last(), Some(&(total, total)));
        assert_eq!(
            cloud.dataless_checks.get(),
            1,
            "the flag is not watched after a complete read"
        );
    }

    #[test]
    fn cancelling_stops_the_wait() {
        let (_dir, path) = file_of(100);
        let cancel = AtomicBool::new(true);

        let before = run(&FakeCloud::new(1_000, false), &path, &cancel);
        assert!(matches!(before.0, Err(Error::Cancelled)));
        assert_eq!(
            before.1,
            [(0, 100)],
            "it reported the start and then stopped"
        );

        let cloud = FakeCloud::new(1_000_000, false);
        let cancel = AtomicBool::new(false);
        let mut polls = 0;
        let result = wait_for_local(
            &path,
            &mut |_, _| {
                polls += 1;
                if polls == 3 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &cancel,
            &cloud,
            Duration::from_millis(1),
        );
        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn cancelling_interrupts_the_fallback_read_between_chunks() {
        let (_dir, path) = file_of(5 * READ_CHUNK);
        let cloud = FakeCloud::new(0, true);
        let cancel = AtomicBool::new(false);
        let mut chunks = 0;

        let result = wait_for_local(
            &path,
            &mut |done, _| {
                if done > 0 {
                    chunks += 1;
                    if chunks == 2 {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            },
            &cancel,
            &cloud,
            Duration::from_millis(1),
        );

        assert!(matches!(result, Err(Error::Cancelled)));
        assert_eq!(chunks, 2);
    }

    #[test]
    fn a_placeholder_whose_file_vanished_is_an_io_error() {
        let (dir, path) = file_of(10);
        fs::remove_file(&path).unwrap();
        // The fake reports it dataless while the file is missing.
        struct Gone;
        impl Storage for Gone {
            fn is_dataless(&self, _: &Path) -> bool {
                true
            }
            fn start_download(&self, _: &Path) -> io::Result<()> {
                Ok(())
            }
            fn bytes_on_disk(&self, _: &Path) -> u64 {
                0
            }
        }

        let result = wait_for_local(
            &path,
            &mut |_, _| {},
            &AtomicBool::new(false),
            &Gone,
            Duration::from_millis(1),
        );

        assert!(matches!(result, Err(Error::Io(_))), "{result:?}");
        drop(dir);
    }
}
