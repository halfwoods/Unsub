//! What the iPod recorded since the last sync, translated back to library ids.

use std::fs;
use std::io;

use serde::Serialize;

use super::mapping::Mapping;
use crate::device::{Ipod, database_path, itunes_dir};
use crate::error::{Error, Result};
use crate::{itunesdb, play_counts};

/// Plays, skips and changes made on the iPod itself since the last sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayDelta {
    pub plays: u32,
    pub skips: u32,
    /// Unix seconds; set when it differs from what the database already said.
    pub last_played: Option<i64>,
    /// 0 to 100; set when the rating was changed on the iPod.
    pub rating: Option<u8>,
}

/// Reads `Play Counts` and matches its entries to tracks this app synced. Tracks without a change
/// are left out, as are tracks the app did not put there. Nothing is modified: call
/// [`clear_play_counts`] once the result is safely merged into the library.
pub fn read_play_counts_for_sync_back(ipod: &Ipod) -> Result<Vec<(i64, PlayDelta)>> {
    let mapping = Mapping::load(&ipod.mount_path)?;
    let library_id_of = mapping.library_id_by_dbid();
    let data = itunesdb::load(
        &database_path(&ipod.mount_path, ipod.database),
        ipod.database,
    )?;
    // The database itself, not merged with the counts, so the deltas are relative to it.
    let (tracks, _) = itunesdb::parse(&data, &ipod.mount_path)?;

    let deltas = play_counts::read(&ipod.mount_path)?
        .into_iter()
        .zip(&tracks)
        .filter_map(|(entry, track)| {
            let library_id = *library_id_of.get(&track.dbid)?;
            let delta = PlayDelta {
                plays: entry.play_count,
                skips: entry.skip_count,
                last_played: entry
                    .last_played
                    .filter(|&played| Some(played) != track.last_played),
                rating: entry
                    .rating
                    .filter(|&rating| rating != 0)
                    .map(|rating| u8::try_from(rating.min(100)).unwrap_or(100))
                    .filter(|&rating| rating != track.rating),
            };
            let changed = delta.plays > 0
                || delta.skips > 0
                || delta.last_played.is_some()
                || delta.rating.is_some();
            changed.then_some((library_id, delta))
        })
        .collect();
    Ok(deltas)
}

/// Deletes the iPod's `Play Counts` file, as iTunes does after syncing it back. Its entries are
/// positions in the track list, so once the database is rewritten they no longer line up and must
/// not be read again. Call it after [`read_play_counts_for_sync_back`]'s result has been saved.
pub fn clear_play_counts(ipod: &Ipod) -> Result<()> {
    super::checksum_key(ipod)?;
    let path = itunes_dir(&ipod.mount_path).join(play_counts::FILE_NAME);
    match fs::remove_file(&path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(Error::io(path, error)),
        _ => {
            super::commit::sync_directory(&path);
            Ok(())
        }
    }
}
