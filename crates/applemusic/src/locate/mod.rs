//! Finding the files of a library whose location the system did not give: when Music's media
//! folder has been moved, or synced to iCloud Drive, or copied to another computer.
//!
//! The files are found from their names alone. Music names what it creates predictably
//! (`Album Artist/Album/01 Title.m4a`), so most songs resolve exactly; names that another tool
//! or an older version wrote are matched more loosely, and anything that would be a guess is
//! left unmatched. No file is ever opened, because in an iCloud folder that downloads it.

mod index;
mod matching;
mod names;
mod tree;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::item::MusicLibraryItem;

/// How sure a match is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    /// The file is in the folder and has the name Music would have given it.
    Exact,
    /// The right album folder, but the file is named differently: another tool, a changed title,
    /// a name cut short; or the folder itself is spelled a little differently.
    Folder,
    /// Not where it should be: the only file in the whole folder with this title.
    Fuzzy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocatedFile {
    pub path: PathBuf,
    pub confidence: Confidence,
    /// The content is in iCloud only; opening the file downloads it.
    pub dataless: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Located {
    /// By `MusicLibraryItem::persistent_id`.
    pub matched: HashMap<String, LocatedFile>,
    /// Songs that should have a file but none was found, in the order given. Songs that already
    /// have their file, and Apple Music streams that never will, are in neither list.
    pub unmatched: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LocateStage {
    /// Listing the folder. The total is not known.
    Scanning,
    Matching,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocateProgress {
    pub stage: LocateStage,
    /// Files listed so far, or songs placed so far.
    pub done: usize,
    pub total: Option<usize>,
}

/// Finds, for each item whose file is not there, the file in `folder` that holds it. `folder` is
/// the one with the artist folders in it, or Music's own `iTunes Media` / `Media.localized`
/// folder, from which the `Music` folder is used.
///
/// A 37,000-song library against 37,000 files takes a few seconds, nearly all of it listing the
/// folder. Nothing is opened, so cloud-only files stay in the cloud; `dataless` says which they
/// are.
pub fn locate_in_folder(
    items: &[MusicLibraryItem],
    folder: &Path,
    progress: &mut dyn FnMut(LocateProgress),
) -> Result<Located> {
    locate_with(&tree::Disk, items, folder, progress)
}

fn locate_with(
    tree: &dyn tree::Tree,
    items: &[MusicLibraryItem],
    folder: &Path,
    progress: &mut dyn FnMut(LocateProgress),
) -> Result<Located> {
    let found = tree::walk(tree, folder, &mut |done| {
        progress(LocateProgress {
            stage: LocateStage::Scanning,
            done,
            total: None,
        });
    })?;
    let index = index::Index::build(found);
    Ok(matching::run(&index, items, progress))
}

#[cfg(test)]
mod tests;
