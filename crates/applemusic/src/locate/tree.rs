//! Reading a folder of songs without ever opening one.
//!
//! In a folder synced from iCloud most files are placeholders, and opening a placeholder
//! downloads it. Everything here therefore works from directory listings and the metadata that
//! comes with them. `Tree` has no way to open a file, so nothing built on it can.

use std::io;
use std::path::{Path, PathBuf};

use crate::error::Result;

/// Music keeps files at `Artist/Album/Track`, sometimes with a disc folder in between.
const MAX_DEPTH: usize = 4;

const AUDIO_EXTENSIONS: [&str; 17] = [
    "mp3", "m4a", "m4p", "m4b", "aac", "aif", "aiff", "wav", "flac", "ogg", "opus", "wma", "mp4",
    "m4v", "caf", "wv", "ape",
];

pub(crate) struct Entry {
    pub name: String,
    pub kind: EntryKind,
}

pub(crate) enum EntryKind {
    Dir,
    File {
        size: u64,
        /// Whether the content lives only in iCloud and would be downloaded when opened.
        dataless: bool,
    },
}

/// A listing of one folder at a time.
pub(crate) trait Tree {
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>>;
}

/// The real filesystem. Links are not followed, and names that are not UTF-8 are skipped.
pub(crate) struct Disk;

impl Tree for Disk {
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let kind = entry.file_type()?;
            if kind.is_dir() {
                entries.push(Entry {
                    name,
                    kind: EntryKind::Dir,
                });
            } else if kind.is_file() {
                // `metadata` of a directory entry is a stat of the file, not a read of it.
                let metadata = entry.metadata()?;
                entries.push(Entry {
                    name,
                    kind: EntryKind::File {
                        size: metadata.len(),
                        dataless: is_dataless(&metadata),
                    },
                });
            }
        }
        Ok(entries)
    }
}

#[cfg(target_os = "macos")]
fn is_dataless(metadata: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    /// `SF_DATALESS` in `<sys/stat.h>`.
    const SF_DATALESS: u32 = 0x4000_0000;
    metadata.st_flags() & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
fn is_dataless(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// An audio file found in the folder.
pub(crate) struct Found {
    /// The folders between the root and the file, outermost first.
    pub folders: Vec<String>,
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub dataless: bool,
}

/// Lists every audio file under `root`, calling `seen` with the running count as it goes.
pub(crate) fn walk(
    tree: &dyn Tree,
    root: &Path,
    seen: &mut dyn FnMut(usize),
) -> Result<Vec<Found>> {
    let root = media_folder(tree, root)?;
    let mut found = Vec::new();
    // The root must be listable; a subfolder that is not is just left out.
    let mut pending = vec![(root.clone(), Vec::new())];
    let mut first = true;
    while let Some((dir, folders)) = pending.pop() {
        let entries = match tree.list(&dir) {
            Ok(entries) => entries,
            Err(error) if first => return Err(error.into()),
            Err(_) => continue,
        };
        first = false;
        for entry in entries {
            match entry.kind {
                EntryKind::Dir if folders.len() + 1 < MAX_DEPTH => {
                    let mut inside = folders.clone();
                    inside.push(entry.name.clone());
                    pending.push((dir.join(&entry.name), inside));
                }
                EntryKind::File { size, dataless } if is_audio(&entry.name) => {
                    found.push(Found {
                        folders: folders.clone(),
                        path: dir.join(&entry.name),
                        name: entry.name,
                        size,
                        dataless,
                    });
                    if found.len() % 500 == 0 {
                        seen(found.len());
                    }
                }
                _ => {}
            }
        }
    }
    seen(found.len());
    // Listing order is arbitrary; a stable order makes ties break the same way every time.
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

/// Music's own folder (`iTunes Media`, `Media.localized`) holds the songs in a `Music` folder
/// next to ones for podcasts, movies and the rest. If that is what was chosen, look inside.
fn media_folder(tree: &dyn Tree, root: &Path) -> Result<PathBuf> {
    const SIBLINGS: [&str; 6] = [
        "podcasts",
        "movies",
        "tv shows",
        "audiobooks",
        "books",
        "automatically add to music",
    ];
    let entries = tree.list(root)?;
    let dirs: Vec<String> = entries
        .into_iter()
        .filter(|entry| matches!(entry.kind, EntryKind::Dir))
        .map(|entry| entry.name)
        .collect();
    let music = dirs.iter().find(|name| name.eq_ignore_ascii_case("music"));
    let beside_siblings = dirs.iter().any(|name| {
        let name = name.to_lowercase();
        SIBLINGS.contains(&name.as_str()) || name == "automatically add to itunes"
    });
    Ok(match music {
        Some(music) if beside_siblings => root.join(music),
        _ => root.to_owned(),
    })
}

fn is_audio(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && AUDIO_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

#[cfg(test)]
pub(crate) mod fake {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    /// A file's size and whether it is dataless; `None` marks a folder.
    type Kind = Option<(u64, bool)>;

    /// A folder tree in memory that remembers every listing asked of it.
    #[derive(Default)]
    pub(crate) struct Fake {
        dirs: HashMap<PathBuf, Vec<(String, Kind)>>,
        pub listed: RefCell<Vec<PathBuf>>,
    }

    impl Fake {
        /// Adds a file at `path` relative to `root`, with its size and whether it is dataless.
        pub fn file(&mut self, root: &Path, path: &str, size: u64, dataless: bool) -> &mut Self {
            let mut dir = root.to_owned();
            let parts: Vec<&str> = path.split('/').collect();
            for (n, part) in parts.iter().enumerate() {
                let last = n + 1 == parts.len();
                let listing = self.dirs.entry(dir.clone()).or_default();
                let known = listing.iter().any(|(name, _)| name == part);
                if !known {
                    listing.push(((*part).to_owned(), last.then_some((size, dataless))));
                }
                dir = dir.join(part);
            }
            self.dirs.entry(root.to_owned()).or_default();
            self
        }

        /// Adds a folder that is listed in its parent but cannot itself be listed.
        pub fn unreadable_folder(&mut self, root: &Path, path: &str) -> &mut Self {
            let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
            let parent = if parent.is_empty() {
                root.to_owned()
            } else {
                root.join(parent)
            };
            self.dirs
                .entry(parent)
                .or_default()
                .push((name.to_owned(), None));
            self
        }
    }

    impl Tree for Fake {
        fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
            self.listed.borrow_mut().push(dir.to_owned());
            let listing = self
                .dirs
                .get(dir)
                .map_or_else(|| Err(io::Error::from(io::ErrorKind::NotFound)), Ok)?;
            Ok(listing
                .iter()
                .map(|(name, file)| Entry {
                    name: name.clone(),
                    kind: match file {
                        Some((size, dataless)) => EntryKind::File {
                            size: *size,
                            dataless: *dataless,
                        },
                        None => EntryKind::Dir,
                    },
                })
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;

    const ROOT: &str = "/music";

    fn names(found: &[Found]) -> Vec<&Path> {
        found.iter().map(|file| file.path.as_path()).collect()
    }

    #[test]
    fn lists_audio_files_at_every_level_music_uses() {
        let mut fake = Fake::default();
        let root = Path::new(ROOT);
        fake.file(root, "Artist/Album/01 Song.mp3", 10, true);
        fake.file(root, "Artist/Album/Disc 2/01 Song.m4a", 20, false);
        fake.file(root, "Artist/loose.flac", 30, true);
        fake.file(root, "stray.wav", 40, true);

        let found = walk(&fake, root, &mut |_| {}).unwrap();

        assert_eq!(
            names(&found),
            [
                Path::new("/music/Artist/Album/01 Song.mp3"),
                Path::new("/music/Artist/Album/Disc 2/01 Song.m4a"),
                Path::new("/music/Artist/loose.flac"),
                Path::new("/music/stray.wav"),
            ]
        );
        assert_eq!(found[0].folders, ["Artist", "Album"]);
        assert_eq!((found[0].size, found[0].dataless), (10, true));
        assert!(!found[1].dataless);
        assert!(found[3].folders.is_empty());
    }

    #[test]
    fn leaves_out_other_files_and_what_is_too_deep() {
        let mut fake = Fake::default();
        let root = Path::new(ROOT);
        for path in [
            "A/B/cover.jpg",
            "A/B/.DS_Store",
            "A/B/notes.txt",
            "A/B/Live.asd",
            "A/B/.mp3",
            "A/B/C/D/E/too deep.mp3",
        ] {
            fake.file(root, path, 1, true);
        }

        assert!(walk(&fake, root, &mut |_| {}).unwrap().is_empty());
    }

    #[test]
    fn extensions_are_recognized_in_any_case() {
        assert!(is_audio("Song.MP3"));
        assert!(is_audio("Song.Flac"));
        assert!(is_audio("A.B.m4a"));
        assert!(!is_audio("Song.mp3.part"));
        assert!(!is_audio("mp3"));
    }

    #[test]
    fn a_folder_that_cannot_be_listed_is_skipped_but_not_the_root() {
        let mut fake = Fake::default();
        let root = Path::new(ROOT);
        fake.file(root, "Good/Album/01 Song.mp3", 1, true);
        fake.unreadable_folder(root, "Bad");

        let found = walk(&fake, root, &mut |_| {}).unwrap();

        assert_eq!(names(&found), [Path::new("/music/Good/Album/01 Song.mp3")]);
        assert!(walk(&fake, Path::new("/elsewhere"), &mut |_| {}).is_err());
    }

    #[test]
    fn the_running_count_is_reported() {
        let mut fake = Fake::default();
        let root = Path::new(ROOT);
        for n in 0..1_200 {
            fake.file(root, &format!("A/B/{n:04} Song.mp3"), 1, true);
        }
        let mut counts = Vec::new();

        walk(&fake, root, &mut |count| counts.push(count)).unwrap();

        assert_eq!(counts, [500, 1_000, 1_200]);
    }

    #[test]
    fn the_music_folder_of_a_media_folder_is_the_root() {
        let mut fake = Fake::default();
        let root = Path::new(ROOT);
        fake.file(root, "Music/A/B/01 Song.mp3", 1, true);
        fake.file(root, "Movies/Film.m4v", 1, true);

        assert_eq!(
            names(&walk(&fake, root, &mut |_| {}).unwrap()),
            [Path::new("/music/Music/A/B/01 Song.mp3")]
        );
    }
}
