use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::mapping::Mapping;
use super::{
    SyncPlan, SyncPlaylist, SyncTrack, checksum_key, require_blank_or_database,
    require_guid_matches,
};
use crate::database::{Database, IpodTrack};
use crate::device::{Ipod, database_exists, database_path};
use crate::error::{Error, Result};
use crate::itunesdb::artwork::Artwork;

/// Working space the database rewrite needs on top of the music: the new database, its backup and
/// (once) the original copy, so three times the current size, plus slack for the bookkeeping file.
pub(super) fn reserve_bytes(mount: &Path, ipod: &Ipod) -> u64 {
    let database_len =
        fs::metadata(database_path(mount, ipod.database)).map_or(0, |meta| meta.len());
    3 * database_len + (8 << 20)
}

/// How a sync gets its space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Space {
    /// There is room for everything while the old tracks are still on the iPod.
    Fits,
    /// There is room only once the tracks being removed are gone, so they go first.
    AfterRemovals,
    /// Not enough even then; carries the space that is free now, without what removals would free.
    Short { available: u64 },
}

pub(super) fn space(needed: u64, reserve: u64, available: u64, freed: u64) -> Space {
    let needed = needed.saturating_add(reserve);
    if needed <= available {
        Space::Fits
    } else if freed > 0 && needed <= available.saturating_add(freed) {
        Space::AfterRemovals
    } else {
        Space::Short { available }
    }
}

/// Bytes the given files occupy now. A file that is already gone frees nothing.
pub(super) fn freed_bytes(files: impl IntoIterator<Item = PathBuf>) -> u64 {
    files
        .into_iter()
        .filter_map(|file| fs::metadata(file).ok())
        .map(|meta| meta.len())
        .sum()
}

/// Space for the thumbnails in `tracks`. Sparse devices can share one image per album and cover
/// key; other devices need an image per track. A cover key is assumed to have a cover, so the
/// estimate errs high.
pub(super) fn artwork_bytes<'a>(
    artwork: Option<&Artwork>,
    tracks: impl IntoIterator<Item = &'a SyncTrack>,
) -> u64 {
    let Some(artwork) = artwork else { return 0 };
    let covers = tracks
        .into_iter()
        .filter_map(|track| track.artwork_key.map(|key| (track.album.as_str(), key)));
    let images = if artwork.shares_images() {
        covers.collect::<HashSet<_>>().len()
    } else {
        covers.count()
    };
    images as u64 * artwork.bytes_per_image()
}

/// Works out what a sync would change. Nothing on the iPod is touched.
///
/// Only tracks recorded in the iPod's `Unsub.json` are candidates for update or removal;
/// music that iTunes put on the iPod is never listed.
pub fn plan_sync(
    ipod: &Ipod,
    current: &Database,
    tracks: &[SyncTrack],
    // Which tracks a playlist holds never changes what has to be copied; the parameter is here so
    // callers pass the same arguments to `plan_sync` and `apply_sync`.
    _playlists: &[SyncPlaylist],
) -> Result<SyncPlan> {
    let key = checksum_key(ipod)?;
    require_blank_or_database(ipod)?;
    if ipod.guid_from_usb && database_exists(ipod) {
        let path = database_path(&ipod.mount_path, ipod.database);
        let database = fs::read(&path).map_err(|source| Error::io(path, source))?;
        require_guid_matches(ipod, key, &database)?;
    }
    let mapping = Mapping::load(&ipod.mount_path)?;
    let artwork = Artwork::for_device(&ipod.mount_path, ipod.model.as_ref());
    let dbid_of = mapping.dbid_by_library_id();
    let art_key_of = mapping.artwork_key_by_library_id();
    let library_id_of = mapping.library_id_by_dbid();
    let on_ipod: HashMap<u64, &IpodTrack> = current
        .tracks
        .iter()
        .map(|track| (track.dbid, track))
        .collect();

    let mut wanted = HashSet::new();
    let (mut add, mut update, mut bytes_needed) = (Vec::new(), Vec::new(), 0u64);
    let mut needing_art = Vec::new();
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
                needing_art.push(track);
            }
            Some(on_ipod) => {
                // A different key means the cover was added or changed since it was last written.
                let new_art = artwork.is_some()
                    && track.artwork_key.is_some()
                    && art_key_of.get(&track.library_id) != Some(&track.artwork_key);
                if differs(track, on_ipod) || new_art {
                    update.push(track.library_id);
                }
                if new_art {
                    needing_art.push(track);
                }
            }
        }
    }
    bytes_needed += artwork_bytes(artwork.as_ref(), needing_art);

    let remove: Vec<u32> = current
        .tracks
        .iter()
        .filter(|track| {
            library_id_of
                .get(&track.dbid)
                .is_some_and(|library_id| !wanted.contains(library_id))
        })
        .map(|track| track.id)
        .collect();
    let freed = freed_bytes(
        current
            .tracks
            .iter()
            .filter(|track| remove.contains(&track.id))
            .map(|track| track.path.clone()),
    );

    let available = fs4::available_space(&ipod.mount_path)
        .map_err(|source| Error::io(&ipod.mount_path, source))?;
    let space = space(
        bytes_needed,
        reserve_bytes(&ipod.mount_path, ipod),
        available,
        freed,
    );
    Ok(SyncPlan {
        add,
        update,
        remove,
        bytes_needed,
        fits: !matches!(space, Space::Short { .. }),
        removals_first: space == Space::AfterRemovals,
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
        || track.sound_check.unwrap_or(0) != on_ipod.sound_check
        || track.play_count != on_ipod.play_count
        || track.last_played != on_ipod.last_played
        || track.sort_title != on_ipod.sort_title
        || track.sort_artist != on_ipod.sort_artist
        || track.sort_album != on_ipod.sort_album
        || track.sort_album_artist != on_ipod.sort_album_artist
}
