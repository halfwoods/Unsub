use std::collections::HashMap;

use crate::{Error, Profile, Result, Storage, storage::TEMP_SUFFIX};

/// What is on the device in the folders a sync writes to, listed once per plan or apply.
pub(crate) struct Inventory {
    /// Lower-case path to size: FAT is case-insensitive, so `Song.flac` and `song.FLAC` are one file.
    files: HashMap<String, u64>,
    /// Half-written files an earlier sync left behind.
    leftovers: Vec<String>,
}

impl Inventory {
    pub fn scan(storage: &dyn Storage, profile: &Profile) -> Result<Self> {
        let mut files = HashMap::new();
        let mut leftovers = Vec::new();
        for dir in [profile.music_dir(), profile.playlist_dir()] {
            for file in storage
                .list(&dir)
                .map_err(|source| Error::io(&dir, source))?
            {
                if is_temp(&file.path) {
                    leftovers.push(file.path);
                } else {
                    files.insert(file.path.to_lowercase(), file.size);
                }
            }
        }
        leftovers.sort();
        leftovers.dedup();
        Ok(Self { files, leftovers })
    }

    pub fn size(&self, path: &str) -> Option<u64> {
        self.files.get(&path.to_lowercase()).copied()
    }

    /// Whether anything is at `lowercase_path`.
    pub fn contains(&self, lowercase_path: &str) -> bool {
        self.files.contains_key(lowercase_path)
    }

    pub fn leftovers(&self) -> &[String] {
        &self.leftovers
    }
}

/// `dir/.name.as-part`, the name `Storage::put` and `write` give a file while it is incomplete.
fn is_temp(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with('.') && name.ends_with(TEMP_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FsStorage, Storage};

    #[test]
    fn lists_both_folders_case_insensitively_and_sets_temp_files_apart() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FsStorage::new(dir.path());
        storage.write("Music/A/Song.flac", b"123").unwrap();
        storage.write("Playlists/Mix.m3u8", b"#EXTM3U").unwrap();
        storage.write("Music/A/.Other.flac.as-part", b"x").unwrap();
        storage.write("Elsewhere/untouched.txt", b"x").unwrap();
        let inventory = Inventory::scan(&storage, &Profile::default()).unwrap();
        assert_eq!(inventory.size("music/a/SONG.FLAC"), Some(3));
        assert!(inventory.contains("playlists/mix.m3u8"));
        assert!(!inventory.contains("elsewhere/untouched.txt"));
        assert_eq!(inventory.leftovers(), ["Music/A/.Other.flac.as-part"]);
    }

    #[test]
    fn overlapping_folders_are_listed_once() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FsStorage::new(dir.path());
        storage
            .write("Music/Playlists/Mix.m3u8", b"#EXTM3U")
            .unwrap();
        let profile = Profile {
            playlist_dir: "Music/Playlists".to_owned(),
            ..Profile::default()
        };
        let inventory = Inventory::scan(&storage, &profile).unwrap();
        assert_eq!(inventory.size("Music/Playlists/Mix.m3u8"), Some(7));
    }
}
