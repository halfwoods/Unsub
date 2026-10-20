//! The files found in the folder, indexed so that finding the one for a song is a lookup.

use std::collections::HashMap;
use std::path::PathBuf;

use super::names::{fold, loose, split_number};
use super::tree::Found;

pub(crate) struct File {
    pub path: PathBuf,
    pub size: u64,
    pub dataless: bool,
    /// Lower case, without the dot.
    pub extension: String,
    /// The track (and disc) number the name starts with, if it has one.
    pub number: Option<(Option<u32>, u32)>,
    /// The name after the number, loosely.
    pub title: String,
    /// The whole name, loosely.
    pub whole: String,
}

/// The files in one `Artist/Album` folder, including any disc folders inside it.
#[derive(Default)]
pub(crate) struct Album {
    pub files: Vec<usize>,
    /// Folded names without extension; more than one file means several formats of one song.
    pub by_stem: HashMap<String, Vec<usize>>,
}

#[derive(Default)]
pub(crate) struct Index {
    pub files: Vec<File>,
    pub albums: Vec<Album>,
    exact: HashMap<(String, String), Vec<usize>>,
    loosely: HashMap<(String, String), Vec<usize>>,
    by_album_name: HashMap<String, Vec<usize>>,
    by_title: HashMap<String, Vec<usize>>,
}

impl Index {
    pub fn build(found: Vec<Found>) -> Self {
        let mut index = Self::default();
        let mut album_ids: HashMap<(String, String), usize> = HashMap::new();
        for file in found {
            let artist = file.folders.first().cloned().unwrap_or_default();
            let album_name = file.folders.get(1).cloned().unwrap_or_default();
            let album = *album_ids
                .entry((artist.clone(), album_name.clone()))
                .or_insert_with(|| {
                    index.albums.push(Album::default());
                    let id = index.albums.len() - 1;
                    index
                        .exact
                        .entry((fold(&artist), fold(&album_name)))
                        .or_default()
                        .push(id);
                    let (artist_loose, album_loose) = (loose(&artist), loose(&album_name));
                    if !artist_loose.is_empty() && !album_loose.is_empty() {
                        index
                            .loosely
                            .entry((artist_loose, album_loose.clone()))
                            .or_default()
                            .push(id);
                    }
                    if !album_loose.is_empty() {
                        index.by_album_name.entry(album_loose).or_default().push(id);
                    }
                    id
                });

            let Some((stem, extension)) = file.name.rsplit_once('.') else {
                continue;
            };
            let (number, title) = match split_number(stem) {
                Some(numbered) => (Some((numbered.disc, numbered.track)), loose(numbered.title)),
                None => (None, loose(stem)),
            };
            let id = index.files.len();
            if !title.is_empty() {
                index.by_title.entry(title.clone()).or_default().push(id);
            }
            index.albums[album].files.push(id);
            index.albums[album]
                .by_stem
                .entry(fold(stem))
                .or_default()
                .push(id);
            index.files.push(File {
                path: file.path,
                size: file.size,
                dataless: file.dataless,
                extension: extension.to_ascii_lowercase(),
                number,
                whole: loose(stem),
                title,
            });
        }
        index
    }

    /// The folders an item might be in, best first: the names Music would give them, then names
    /// that agree once case, accents and punctuation are ignored, then the one folder anywhere
    /// with that album name. The `bool` is whether the folder is the one Music would have made.
    pub fn albums_for(&self, artists: &[String], album: &str) -> Vec<(usize, bool)> {
        let mut found: Vec<(usize, bool)> = Vec::new();
        let mut add = |ids: Option<&Vec<usize>>, exact: bool| {
            for &id in ids.into_iter().flatten() {
                if !found.iter().any(|(known, _)| *known == id) {
                    found.push((id, exact));
                }
            }
        };
        let album_fold = fold(album);
        for artist in artists {
            add(self.exact.get(&(fold(artist), album_fold.clone())), true);
        }
        let album_loose = loose(album);
        if !album_loose.is_empty() {
            for artist in artists {
                let key = (loose(artist), album_loose.clone());
                add(self.loosely.get(&key), false);
            }
            // Only when there is exactly one such album folder is it safe to assume.
            if let Some([only]) = self.by_album_name.get(&album_loose).map(Vec::as_slice) {
                add(Some(&vec![*only]), false);
            }
        }
        found
    }

    /// Every file whose name, after any track number, is this loosely written title.
    pub fn with_title(&self, title: &str) -> &[usize] {
        self.by_title.get(title).map_or(&[], Vec::as_slice)
    }
}
