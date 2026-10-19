use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use super::mapping::Mapping;
use super::{SyncPlan, SyncPlaylist, SyncTrack, checksum_key};
use crate::database::{Database, IpodTrack};
use crate::device::{Ipod, database_path};
use crate::error::{Error, Result};

/// Working space the database rewrite needs on top of the music: the new database, its backup and
/// (once) the original copy, so three times the current size, plus slack for the bookkeeping file.
pub(super) fn reserve_bytes(mount: &Path, ipod: &Ipod) -> u64 {
    let database_len =
        fs::metadata(database_path(mount, ipod.database)).map_or(0, |meta| meta.len());
    3 * database_len + (8 << 20)
}

/// Works out what a sync would change. Nothing on the iPod is touched.
///
/// Only tracks recorded in the iPod's `AntiStreaming.json` are candidates for update or removal;
/// music that iTunes put on the iPod is never listed.
pub fn plan_sync(
    ipod: &Ipod,
    current: &Database,
    tracks: &[SyncTrack],
    // Which tracks a playlist holds never changes what has to be copied; the parameter is here so
    // callers pass the same arguments to `plan_sync` and `apply_sync`.
    _playlists: &[SyncPlaylist],
) -> Result<SyncPlan> {
    checksum_key(ipod)?;
    let mapping = Mapping::load(&ipod.mount_path)?;
    let dbid_of = mapping.dbid_by_library_id();
    let library_id_of = mapping.library_id_by_dbid();
    let on_ipod: HashMap<u64, &IpodTrack> = current
        .tracks
        .iter()
        .map(|track| (track.dbid, track))
        .collect();

    let mut wanted = HashSet::new();
    let (mut add, mut update, mut bytes_needed) = (Vec::new(), Vec::new(), 0u64);
    for track in tracks
        .iter()
        .filter(|track| wanted.insert(track.library_id))
    {
        let existing = dbid_of
            .get(&track.library_id)
            .and_then(|dbid| on_ipod.get(dbid));
        match existing {
            None => {
                let size = fs::metadata(&track.source_path)
                    .map_err(|source| Error::io(&track.source_path, source))?
                    .len();
                bytes_needed += size;
                add.push(track.library_id);
            }
            Some(on_ipod) if differs(track, on_ipod) => update.push(track.library_id),
            Some(_) => {}
        }
    }

    let remove = current
        .tracks
        .iter()
        .filter(|track| {
            library_id_of
                .get(&track.dbid)
                .is_some_and(|library_id| !wanted.contains(library_id))
        })
        .map(|track| track.id)
        .collect();

    let available = fs4::available_space(&ipod.mount_path)
        .map_err(|source| Error::io(&ipod.mount_path, source))?;
    let fits = bytes_needed.saturating_add(reserve_bytes(&ipod.mount_path, ipod)) <= available;
    Ok(SyncPlan {
        add,
        update,
        remove,
        bytes_needed,
        fits,
    })
}

/// Compares only what a sync can change: tags, rating and play statistics. The audio properties
/// belong to the file already on the iPod.
fn differs(track: &SyncTrack, on_ipod: &IpodTrack) -> bool {
    track.title != on_ipod.title
        || track.artist != on_ipod.artist
        || track.album_artist != on_ipod.album_artist
        || track.album != on_ipod.album
        || track.genre != on_ipod.genre
        || track.composer != on_ipod.composer
        || track.year != on_ipod.year
        || track.track_number != on_ipod.track_number
        || track.track_count != on_ipod.track_count
        || track.disc_number != on_ipod.disc_number
        || track.disc_count != on_ipod.disc_count
        || track.rating.min(100) != on_ipod.rating
        || track.compilation != on_ipod.compilation
        || track.play_count != on_ipod.play_count
        || track.last_played != on_ipod.last_played
        || track.sort_title != on_ipod.sort_title
        || track.sort_artist != on_ipod.sort_artist
        || track.sort_album != on_ipod.sort_album
        || track.sort_album_artist != on_ipod.sort_album_artist
}
