//! Playlists made on the iPod itself with its On-The-Go menu. The iPod keeps them outside the
//! database, in `OTGPlaylistInfo`, `OTGPlaylistInfo_1`, ... in the iTunes folder; each lists the
//! positions of its tracks in the iTunesDB track list. Layout and numbering follow libgpod's
//! `read_OTG_playlists` and `process_OTG_file` (itdb_itunesdb.c), which reads nothing of them from
//! the database itself.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::commit::sync_directory;
use super::mapping::Mapping;
use crate::bytes::tag_at;
use crate::database::Database;
use crate::device::{Ipod, itunes_dir};
use crate::error::{Error, Result};

const FILE_NAME: &str = "OTGPlaylistInfo";
/// The headers and entries libgpod knows are this long; longer ones carry extra data it ignores.
const MIN_HEADER_LEN: usize = 0x14;
const MIN_ENTRY_LEN: usize = 4;

/// A playlist made on the iPod, with the tracks in it that this app put there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnTheGo {
    /// `On-The-Go 1`, `On-The-Go 2`, ... in the order of the iPod's files, so a playlist keeps its
    /// number when an earlier one has no tracks of this app's.
    pub name: String,
    /// `library_id`s in playlist order. Tracks that iTunes or another tool put on the iPod are left
    /// out, as the library does not have them.
    pub tracks: Vec<i64>,
}

/// Reads the iPod's On-The-Go playlists. `database` must be the one read from this iPod (the
/// files count positions in it); playlists left with no track of this app's are not returned.
///
/// Nothing is modified: once the playlists are saved to the library, call [`clear_on_the_go`].
/// A file that does not fit the database (a position past its last track) is an error rather than
/// a guess at which tracks were meant.
pub fn read_on_the_go(ipod: &Ipod, database: &Database) -> Result<Vec<OnTheGo>> {
    let library_id_of = Mapping::load(&ipod.mount_path)?.library_id_by_dbid();
    let mut playlists = Vec::new();
    for (number, path) in contiguous(files(&ipod.mount_path)?) {
        let bytes = fs::read(&path).map_err(|source| Error::io(&path, source))?;
        let tracks: Vec<i64> = positions(&bytes, &path, database.tracks.len())?
            .into_iter()
            .filter_map(|position| {
                library_id_of
                    .get(&database.tracks.get(position)?.dbid)
                    .copied()
            })
            .collect();
        if !tracks.is_empty() {
            playlists.push(OnTheGo {
                name: format!("On-The-Go {number}"),
                tracks,
            });
        }
    }
    Ok(playlists)
}

/// Deletes the iPod's On-The-Go playlist files, as iTunes does once it has imported them. They
/// count positions in the database, so they must not outlive a change to its track list: call this
/// after [`read_on_the_go`]'s result has been saved.
pub fn clear_on_the_go(ipod: &Ipod) -> Result<()> {
    super::checksum_key(ipod)?;
    let files = files(&ipod.mount_path)?;
    for (_, path) in &files {
        match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(Error::io(path, error));
            }
            _ => {}
        }
    }
    if let Some((_, path)) = files.first() {
        sync_directory(path);
    }
    Ok(())
}

/// Whether the iPod has any On-The-Go playlist file.
pub(super) fn present(mount: &Path) -> bool {
    files(mount).is_ok_and(|files| !files.is_empty())
}

/// The playlist files by playlist number: `OTGPlaylistInfo` is 1, `OTGPlaylistInfo_1` is 2. Names
/// are matched without regard to case, as libgpod's path lookup does for the iPod's filesystems.
fn files(mount: &Path) -> Result<Vec<(u32, PathBuf)>> {
    let dir = itunes_dir(mount);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(Error::io(dir, source)),
    };
    let mut found: Vec<(u32, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_ascii_lowercase();
            let rest = name.strip_prefix(&FILE_NAME.to_ascii_lowercase())?;
            let number = match rest.strip_prefix('_') {
                None if rest.is_empty() => 1,
                Some(digits) if digits.bytes().all(|byte| byte.is_ascii_digit()) => {
                    digits.parse::<u32>().ok()?.checked_add(1)?
                }
                _ => return None,
            };
            Some((number, entry.path()))
        })
        .collect();
    found.sort();
    Ok(found)
}

/// libgpod stops at the first missing file in the sequence.
fn contiguous(files: Vec<(u32, PathBuf)>) -> impl Iterator<Item = (u32, PathBuf)> {
    let mut expected = 1;
    files.into_iter().take_while(move |(number, _)| {
        let next = *number == expected;
        expected += 1;
        next
    })
}

/// The track positions one file lists. Files written for mobile phones are big endian and say so
/// by their tag being reversed.
fn positions(bytes: &[u8], file: &Path, track_count: usize) -> Result<Vec<usize>> {
    let bad = |offset: usize, reason: &str| {
        Error::corrupt(
            offset,
            format!("On-The-Go playlist {}: {reason}", file.display()),
        )
    };
    let big_endian = match tag_at(bytes, 0) {
        Some(tag) if &tag == b"mhpo" => false,
        Some(tag) if &tag == b"opmh" => true,
        _ => return Err(bad(0, "missing mhpo header")),
    };
    let word = |at: usize| -> Option<usize> {
        let field: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
        let value = if big_endian {
            u32::from_be_bytes(field)
        } else {
            u32::from_le_bytes(field)
        };
        usize::try_from(value).ok()
    };
    let header_len = word(4).ok_or_else(|| bad(4, "truncated"))?;
    if header_len < MIN_HEADER_LEN {
        return Err(bad(4, "header shorter than expected"));
    }
    let entry_len = word(8).ok_or_else(|| bad(8, "truncated"))?;
    if entry_len < MIN_ENTRY_LEN {
        return Err(bad(8, "entries shorter than expected"));
    }
    let count = word(12).ok_or_else(|| bad(12, "truncated"))?;
    (0..count)
        .map(|index| {
            let at = entry_len
                .checked_mul(index)
                .and_then(|offset| offset.checked_add(header_len))
                .ok_or_else(|| bad(header_len, "truncated"))?;
            let position = word(at).ok_or_else(|| bad(at, "truncated"))?;
            if position < track_count {
                Ok(position)
            } else {
                Err(bad(
                    at,
                    &format!("refers to track {position}, which the iPod does not have"),
                ))
            }
        })
        .collect()
}
