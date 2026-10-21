//! Music files on the iPod: where they go, copying them, removing them, and recognising what they
//! contain. iTunes keeps them in `iPod_Control/Music/F00` to `F49` under random four-letter names.

use std::collections::HashSet;
use std::collections::hash_map::RandomState;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use crate::device::{Ipod, itunes_dir};
use crate::error::{Error, Result};
use crate::itunesdb::raw::RawTrack;
use crate::itunesdb::{self, Codec};
use crate::models;

const NAME_LETTERS: usize = 4;
/// Give up on finding an unused name after this many tries per folder choice; with 26^4 names a
/// folder is only this crowded if something is badly wrong.
const NAME_ATTEMPTS: usize = 64;

pub(super) fn music_root(mount: &Path) -> PathBuf {
    mount.join("iPod_Control").join("Music")
}

/// The `F00`-style folders that exist. iTunes pre-creates them; creating more is not this crate's
/// call because the number the firmware looks in is model-specific.
pub(super) fn music_folders(mount: &Path) -> Result<Vec<PathBuf>> {
    let root = music_root(mount);
    let no_folders = || {
        Error::SyncRefused(format!(
            "{} has no F00-style folders to put music in",
            root.display()
        ))
    };
    let entries = fs::read_dir(&root).map_err(|source| match source.kind() {
        // An iPod that was never set up by iTunes may lack the folder altogether.
        io::ErrorKind::NotFound => no_folders(),
        _ => Error::io(&root, source),
    })?;
    let mut folders: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let digits = name.strip_prefix('F').unwrap_or_default();
            digits.len() == 2 && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    if folders.is_empty() {
        return Err(no_folders());
    }
    Ok(folders)
}

/// Whether any file is in the music folders. A name starting with a dot, which is what Finder
/// leaves on a FAT disk it has looked at, is not music.
pub(super) fn has_music(mount: &Path) -> bool {
    let Ok(folders) = music_folders(mount) else {
        return false;
    };
    folders.iter().any(|folder| {
        fs::read_dir(folder).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        })
    })
}

/// How many `Fnn` folders an iPod without any gets, as libgpod's `itdb_create_directories` decides:
/// its model's number, and without a known model 20 below 20 GB of capacity and 50 above.
fn music_dir_count(ipod: &Ipod) -> usize {
    let by_model = (ipod.model.as_ref())
        .and_then(|model| models::music_dirs(&model.model_number))
        .filter(|&count| count > 0);
    by_model.unwrap_or(if ipod.capacity_bytes < 20 << 30 {
        20
    } else {
        50
    })
}

/// [`music_folders`] for an iPod that has no database yet, which may have none of them either: they
/// are made first, the way libgpod initialises a blank iPod.
pub(super) fn music_folders_or_new(ipod: &Ipod) -> Result<Vec<PathBuf>> {
    match music_folders(&ipod.mount_path) {
        Err(Error::SyncRefused(_)) => {
            let root = music_root(&ipod.mount_path);
            for number in 0..music_dir_count(ipod) {
                let folder = root.join(format!("F{number:02}"));
                fs::create_dir_all(&folder).map_err(|source| Error::io(&folder, source))?;
            }
            music_folders(&ipod.mount_path)
        }
        found => found,
    }
}

/// xorshift64*, seeded from the OS-randomised hasher keys: names and ids need to be unpredictable
/// enough not to collide, not cryptographic.
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new() -> Self {
        Self(RandomState::new().build_hasher().finish() | 1)
    }

    pub(super) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub(super) fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % bound as u64).unwrap_or(0)
    }
}

/// A freshly created, empty file with an unused random name in a random music folder.
pub(super) struct Placed {
    pub(super) file: File,
    pub(super) path: PathBuf,
}

/// `create_new` makes the name check and the creation one step, so a name can never be handed out
/// twice, whether by this run or by anything else writing to the iPod.
pub(super) fn place(folders: &[PathBuf], extension: &str, rng: &mut Rng) -> io::Result<Placed> {
    for _ in 0..NAME_ATTEMPTS {
        let folder = &folders[rng.below(folders.len())];
        let name: String = (0..NAME_LETTERS)
            .map(|_| char::from(b'A' + u8::try_from(rng.below(26)).unwrap_or(0)))
            .collect();
        let path = folder.join(format!("{name}.{extension}"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok(Placed { file, path }),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused file name found",
    ))
}

/// Copies `source` into the placed file and flushes it to the device. On failure the partial file
/// is removed so it cannot be mistaken for music later.
pub(super) fn copy_into(source: &Path, placed: Placed) -> io::Result<()> {
    let Placed { mut file, path } = placed;
    let copied = File::open(source)
        .and_then(|mut source| io::copy(&mut source, &mut file))
        .and_then(|_| file.sync_all());
    if copied.is_err() {
        drop(file);
        let _ = fs::remove_file(&path);
    }
    copied
}

/// The list of music files a sync is in the middle of copying, one path per line relative to the
/// mount. It exists only between the first copy of a batch and that batch's commit, so if one is
/// found at the start of a sync, the one before it died with music on the iPod no database lists.
const PENDING_FILE: &str = "Unsub.pending";

fn pending_path(mount: &Path) -> PathBuf {
    itunes_dir(mount).join(PENDING_FILE)
}

/// Writes down, before the copy goes into it, that `file` is about to hold music. A list that
/// cannot be kept costs a crash its cleanup, not the sync.
pub(super) fn note_pending(mount: &Path, file: &Path) {
    let relative = file.strip_prefix(mount).unwrap_or(file);
    let parts: Vec<_> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect();
    let noted = OpenOptions::new()
        .create(true)
        .append(true)
        .open(pending_path(mount))
        .and_then(|mut list| {
            writeln!(list, "{}", parts.join("/"))?;
            list.sync_data()
        });
    let _ = noted;
}

/// Forgets the list, once what it names is in a committed database or has been taken back out.
pub(super) fn clear_pending(mount: &Path) {
    let _ = fs::remove_file(pending_path(mount));
}

/// Deletes the music files a sync that did not finish left behind: those its list names that
/// `listed` (the files the iPod's database refers to) does not hold. Then the list goes.
pub(super) fn sweep_pending(mount: &Path, listed: &HashSet<PathBuf>) {
    let Ok(text) = fs::read_to_string(pending_path(mount)) else {
        return;
    };
    for line in text.lines().filter(|line| !line.is_empty()) {
        let file = line
            .split('/')
            .fold(mount.to_path_buf(), |path, part| path.join(part));
        if !listed.contains(&file) {
            // Only ever from under the music folders, whatever the list says.
            remove_music_file(mount, &file);
        }
    }
    clear_pending(mount);
}

/// Where the database says this track's file is.
pub(super) fn music_path(mount: &Path, track: &RawTrack) -> Option<PathBuf> {
    let path = track.text(itunesdb::PATH).ok().flatten()?;
    Some(itunesdb::native_path(mount, &path))
}

/// Removes a music file, but only one that really lives under the iPod's music folders: the path
/// comes from a database file that could be damaged, and nothing else may ever be deleted.
pub(super) fn remove_music_file(mount: &Path, path: &Path) {
    let inside = path.strip_prefix(music_root(mount)).is_ok_and(|relative| {
        relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    });
    if inside {
        let _ = fs::remove_file(path);
    }
}

/// What iPods of this era can play. A file with any other extension would be copied, listed, and
/// then skipped by the firmware, so it is refused instead.
const PLAYABLE_EXTENSIONS: [&str; 10] = [
    "mp3", "m4a", "m4b", "aac", "wav", "aif", "aiff", "m4v", "mp4", "mov",
];

pub(super) fn is_playable_extension(extension: &str) -> bool {
    PLAYABLE_EXTENSIONS.contains(&extension)
}

/// What the file holds, from its extension and, for MPEG-4 audio, from its sample entry: iTunes
/// labels AAC and Apple Lossless differently although both are `.m4a`.
pub(super) fn detect_codec(path: &Path, extension: &str) -> Codec {
    match extension {
        "mp3" => Codec::Mp3,
        "wav" => Codec::Wav,
        "aif" | "aiff" => Codec::Aiff,
        "m4a" | "m4b" | "mp4" => {
            let sniffed = sample_entry(path, &AUDIO_ENTRIES);
            match (sniffed.as_ref(), extension) {
                (Some(b"alac"), _) => Codec::Alac,
                (Some(_), _) | (None, "m4a" | "m4b") => Codec::Aac,
                (None, _) => Codec::Other,
            }
        }
        _ => Codec::Other,
    }
}

/// Atoms that only contain other atoms on the way from `moov` to a sample description.
const CONTAINERS: [&[u8; 4]; 4] = [b"moov", b"trak", b"mdia", b"minf"];
const MAX_DEPTH: u32 = 8;
const AUDIO_ENTRIES: [&[u8; 4]; 2] = [b"alac", b"mp4a"];
/// FairPlay's sample description, the generic one for encrypted audio, and Audible's: any of them
/// means the audio cannot be played without the keys of the account that bought it.
const PROTECTED_ENTRIES: [&[u8; 4]; 3] = [b"drms", b"enca", b"aavd"];

/// Whether an MPEG-4 file holds encrypted audio. The extension and the database's file type say
/// so for the files iTunes sold, but only the file knows when a protected one has been renamed.
pub(super) fn has_protected_audio(path: &Path) -> bool {
    sample_entry(path, &PROTECTED_ENTRIES).is_some()
}

/// The four-character code of the first sample description among `wanted`, found by opening the
/// file read-only.
fn sample_entry(path: &Path, wanted: &[&[u8; 4]]) -> Option<[u8; 4]> {
    let mut file = File::open(path).ok()?;
    let end = file.metadata().ok()?.len();
    walk_for_entry(&mut file, 0, end, 0, wanted).ok().flatten()
}

/// Walks the MP4 atoms in `start..end` for the first sample description whose four-character
/// code is one of `wanted`.
fn walk_for_entry<F: Read + Seek>(
    file: &mut F,
    start: u64,
    end: u64,
    depth: u32,
    wanted: &[&[u8; 4]],
) -> io::Result<Option<[u8; 4]>> {
    let mut at = start;
    while depth < MAX_DEPTH && at.saturating_add(8) <= end {
        file.seek(SeekFrom::Start(at))?;
        let mut head = [0u8; 8];
        file.read_exact(&mut head)?;
        let size32 = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
        let kind = [head[4], head[5], head[6], head[7]];
        let (header_len, size) = match size32 {
            0 => (8, end - at),
            1 => {
                let mut large = [0u8; 8];
                file.read_exact(&mut large)?;
                (16, u64::from_be_bytes(large))
            }
            size => (8, size),
        };
        if size < header_len || at.saturating_add(size) > end {
            return Ok(None);
        }
        let body = (at + header_len, at + size);
        if CONTAINERS.contains(&&kind) || &kind == b"stbl" {
            if let Some(found) = walk_for_entry(file, body.0, body.1, depth + 1, wanted)? {
                return Ok(Some(found));
            }
        } else if &kind == b"stsd" && body.1 - body.0 >= 16 {
            // Version and flags, entry count, then the first entry: its size and type.
            file.seek(SeekFrom::Start(body.0 + 12))?;
            let mut entry = [0u8; 4];
            file.read_exact(&mut entry)?;
            if wanted.contains(&&entry) {
                return Ok(Some(entry));
            }
        }
        at += size;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::device::DatabaseFormat;
    use crate::models::{IpodFamily, IpodModel, SyncSupport};

    fn ipod_of(model_number: Option<&str>, capacity_gb: u64) -> Ipod {
        Ipod {
            id: String::new(),
            name: String::new(),
            mount_path: PathBuf::new(),
            model: model_number.map(|number| IpodModel {
                family: IpodFamily::Unknown,
                generation: 0,
                name: String::new(),
                capacity_gb: None,
                color: None,
                model_number: number.to_owned(),
            }),
            capacity_bytes: capacity_gb << 30,
            free_bytes: 0,
            firewire_guid: None,
            guid_from_usb: false,
            serial_number: None,
            database: DatabaseFormat::ITunesDb,
            sync: SyncSupport::Supported,
        }
    }

    #[test]
    fn a_blank_ipod_gets_its_models_folders_else_twenty_or_fifty_by_capacity() {
        assert_eq!(music_dir_count(&ipod_of(Some("C293"), 160)), 50);
        assert_eq!(music_dir_count(&ipod_of(Some("9800"), 6)), 6);
        assert_eq!(music_dir_count(&ipod_of(Some("ZZZZ"), 8)), 20);
        assert_eq!(music_dir_count(&ipod_of(None, 19)), 20);
        assert_eq!(music_dir_count(&ipod_of(None, 20)), 50);
    }

    fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut atom = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        atom.extend_from_slice(kind);
        atom.extend_from_slice(body);
        atom
    }

    fn mp4_with_entry(entry: &[u8; 4]) -> Vec<u8> {
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend_from_slice(&36u32.to_be_bytes());
        stsd.extend_from_slice(entry);
        let stbl = atom(b"stbl", &atom(b"stsd", &stsd));
        let moov = atom(
            b"moov",
            &atom(b"trak", &atom(b"mdia", &atom(b"minf", &stbl))),
        );
        [
            atom(b"ftyp", b"M4A \0\0\0\0"),
            atom(b"mdat", &[1, 2, 3]),
            moov,
        ]
        .concat()
    }

    fn sniff_for(bytes: Vec<u8>, wanted: &[&[u8; 4]]) -> Option<[u8; 4]> {
        let len = bytes.len() as u64;
        walk_for_entry(&mut Cursor::new(bytes), 0, len, 0, wanted).unwrap()
    }

    fn sniff(bytes: Vec<u8>) -> Option<[u8; 4]> {
        sniff_for(bytes, &AUDIO_ENTRIES)
    }

    #[test]
    fn finds_apple_lossless_and_aac_sample_entries() {
        assert_eq!(sniff(mp4_with_entry(b"alac")), Some(*b"alac"));
        assert_eq!(sniff(mp4_with_entry(b"mp4a")), Some(*b"mp4a"));
    }

    #[test]
    fn finds_encrypted_sample_entries_only_when_asked_for() {
        for entry in [b"drms", b"enca", b"aavd"] {
            assert_eq!(
                sniff_for(mp4_with_entry(entry), &PROTECTED_ENTRIES),
                Some(*entry)
            );
            assert_eq!(sniff(mp4_with_entry(entry)), None);
        }
        assert_eq!(sniff_for(mp4_with_entry(b"mp4a"), &PROTECTED_ENTRIES), None);
    }

    #[test]
    fn a_video_entry_or_garbage_yields_nothing_and_never_loops() {
        assert_eq!(sniff(mp4_with_entry(b"avc1")), None);
        assert_eq!(sniff(vec![0xff; 64]), None);
        assert_eq!(sniff(vec![0, 0, 0, 0, b'm', b'o', b'o', b'v']), None);
        assert_eq!(sniff(Vec::new()), None);
    }

    #[test]
    fn music_files_are_only_removed_from_under_the_music_folders() {
        let mount = tempfile::tempdir().unwrap();
        let inside = music_root(mount.path()).join("F00").join("ABCD.mp3");
        fs::create_dir_all(inside.parent().unwrap()).unwrap();
        fs::write(&inside, b"x").unwrap();
        let outside = mount.path().join("keep.txt");
        fs::write(&outside, b"x").unwrap();
        let sneaky = music_root(mount.path())
            .join("F00")
            .join("..")
            .join("..")
            .join("..")
            .join("keep.txt");

        remove_music_file(mount.path(), &outside);
        remove_music_file(mount.path(), &sneaky);
        remove_music_file(mount.path(), &inside);

        assert!(outside.exists());
        assert!(!inside.exists());
    }
}
