//! Undoing the latest sync: the database it replaced is put back, and the bookkeeping that went
//! with it is made to match.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use super::apply::probe_writable;
use super::commit;
use super::files;
use super::mapping::{MappedTrack, Mapping};
use super::{checksum_key, on_the_go, shuffle_format};
use crate::device::{Ipod, database_path, itunes_dir};
use crate::error::{Error, Result};
use crate::itunesdb::raw::{RawDb, RawPlaylist};
use crate::itunesdb::{self, shuffle};
use crate::play_counts;

/// Puts back the database the latest [`super::apply_sync`] replaced, which it kept as
/// `iTunesDB.unsub-backup`. The backup is the database as it was before that sync, so
/// whatever iTunes or anything else did to the iPod since is undone with it.
///
/// What goes with it, so the next sync starts from the truth:
/// * `Unsub.json` forgets the tracks and playlists that are not in the restored database,
///   and forgets the cover of any track whose artwork link the restored database does not share
///   with the one it replaces, so the next sync looks at that cover again;
/// * music files that the database being replaced lists for tracks this app added, and the
///   restored database does not list, are deleted. A file is deleted only if the bookkeeping file
///   says this app put it there; music from iTunes is never touched;
/// * on a shuffle, the iTunesSD is written again from the restored database.
///
/// Refused, with nothing changed, when:
/// * the device is one [`super::plan_sync`] would refuse, or there is no backup, or it is not a
///   database this crate reads, or its checksum is not the one this iPod needs;
/// * the iPod has a `Play Counts` or On-The-Go playlist file, which count positions in the
///   database now on it and would point at the wrong tracks in the restored one: read and clear
///   them first;
/// * the sync being undone deleted music that the backup still lists, so restoring would list
///   songs that cannot play.
///
/// The ArtworkDB is not restored: it only ever gains images (one is dropped a sync after nothing
/// refers to it), so it still holds every image the backup points at. Calling this twice is
/// harmless.
pub fn restore_backup(ipod: &Ipod) -> Result<()> {
    let checksum = checksum_key(ipod)?;
    let format = shuffle_format(ipod)?;
    let mount = ipod.mount_path.as_path();
    probe_writable(mount)?;
    let database = database_path(mount, ipod.database);
    let backup = commit::backup_of(&database);
    let refuse = |reason: String| Err(Error::SyncRefused(format!("{reason}; nothing was changed")));

    let bytes = match fs::read(&backup) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return refuse("there is no backup of the iPod's database to restore".to_owned());
        }
        Err(source) => return Err(Error::io(backup, source)),
    };
    let restored = match RawDb::parse(&bytes) {
        Ok(restored) => restored,
        Err(error) => {
            return refuse(format!(
                "the backup of the iPod's database is unreadable ({error})"
            ));
        }
    };
    if let Some(key) = &checksum
        && itunesdb::verify_checksum(&bytes, key) != Some(true)
    {
        return refuse("the backup's checksum is not the one this iPod needs".to_owned());
    }
    if itunes_dir(mount).join(play_counts::FILE_NAME).exists() || on_the_go::present(mount) {
        return Err(Error::Conflict(
            "the iPod has a Play Counts or On-The-Go playlist file, which count tracks by position \
             in the database now on it; read them with read_play_counts_for_sync_back and \
             read_on_the_go and delete them with clear_play_counts and clear_on_the_go before \
             restoring; nothing was changed"
                .to_owned(),
        ));
    }

    let mapping = Mapping::load(mount)?;
    // A damaged database is a reason to restore, so failing to read it does not stop this.
    let live = fs::read(&database)
        .ok()
        .and_then(|bytes| RawDb::parse(&bytes).ok());
    let music_of = |db: &RawDb| -> HashMap<u64, PathBuf> {
        db.tracks
            .iter()
            .filter_map(|track| Some((track.dbid(), files::music_path(mount, track)?)))
            .collect()
    };
    let restored_music = music_of(&restored);
    let live_music = live.as_ref().map(music_of);
    if let Some(live_music) = &live_music {
        let gone = restored_music
            .iter()
            .filter(|(dbid, file)| !live_music.contains_key(dbid) && !file.is_file())
            .count();
        if gone > 0 {
            return refuse(format!(
                "the sync being undone deleted the music of {gone} track(s) that the backup lists"
            ));
        }
    }

    let shadow = itunes_dir(mount).join(shuffle::FILE_NAME);
    let mut staged_files = vec![(database.as_path(), bytes)];
    if let Some(format) = format {
        let limit = fs::read(&shadow).map_or(0, |existing| shuffle::volume_limit(&existing));
        staged_files.push((shadow.as_path(), shuffle::emit(&restored, format, limit)?));
    }
    // After the databases: a bookkeeping file that still names tracks the database no longer has
    // is tolerated by a sync, one that has forgotten tracks the database has is not.
    let mapping_path = Mapping::path(mount);
    let reconciled = reconcile(&mapping, &restored, live.as_ref());
    staged_files.push((mapping_path.as_path(), reconciled.to_json()?));
    commit::commit_all(commit::stage_all(&staged_files)?)?;

    let owned: HashSet<u64> = mapping.tracks.iter().map(|mapped| mapped.dbid).collect();
    let still_listed: HashSet<&PathBuf> = restored_music.values().collect();
    for (dbid, file) in live_music.iter().flatten() {
        if owned.contains(dbid)
            && !restored_music.contains_key(dbid)
            && !still_listed.contains(file)
        {
            files::remove_music_file(mount, file);
        }
    }
    Ok(())
}

/// The bookkeeping for the restored database: what it still has, with covers re-examined where
/// the restore changed which image a track points at.
fn reconcile(mapping: &Mapping, restored: &RawDb, live: Option<&RawDb>) -> Mapping {
    let links = |db: &RawDb| -> HashMap<u64, u32> {
        db.tracks
            .iter()
            .map(|track| (track.dbid(), track.artwork_link()))
            .collect()
    };
    let restored_links = links(restored);
    let live_links = live.map(links);
    let tracks = mapping
        .tracks
        .iter()
        .filter(|mapped| restored_links.contains_key(&mapped.dbid))
        .map(|mapped| {
            let cover_unchanged = live_links
                .as_ref()
                .is_some_and(|live| live.get(&mapped.dbid) == restored_links.get(&mapped.dbid));
            MappedTrack {
                artwork_key: mapped.artwork_key.filter(|_| cover_unchanged),
                ..mapped.clone()
            }
        })
        .collect();
    let playlist_ids: HashSet<u64> = restored.playlists.iter().map(RawPlaylist::id).collect();
    let playlists = mapping
        .playlists
        .iter()
        .filter(|playlist| playlist_ids.contains(&playlist.id))
        .cloned()
        .collect();
    Mapping::new(tracks, playlists, mapping.artwork.clone())
}
