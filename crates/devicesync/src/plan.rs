//! Working out what a sync changes, without changing anything.

use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsStr,
    path::Path,
};

use serde::Serialize;

use crate::{
    Device, Error, Profile, Result, SyncItem, SyncPlaylist,
    inventory::Inventory,
    layout::{Template, first_free, lowercase_set, same_destination},
    manifest::Manifest,
    storage::{self, Target},
};

/// Space kept free after a sync: FAT needs room for its tables and a player for its own database.
pub const RESERVE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    pub library_id: i64,
    /// Where the file goes, relative to the device root.
    pub path: String,
    /// The file it replaces at another path (the track's name changed), deleted once the new copy
    /// is in place.
    pub replaces: Option<String>,
    /// Bytes of the source, which a conversion may change.
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedFile {
    pub library_id: i64,
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlan {
    /// Tracks not on the device yet, or whose copy has gone missing.
    pub add: Vec<PlannedFile>,
    /// Tracks on the device whose source changed, or whose name changed.
    pub update: Vec<PlannedFile>,
    /// Tracks this app put on the device that are no longer wanted.
    pub remove: Vec<RemovedFile>,
    pub unchanged: usize,
    /// Playlists to write, and playlists this app wrote earlier that are no longer wanted.
    pub write_playlists: Vec<String>,
    pub remove_playlists: Vec<String>,
    /// Bytes of the sources of `add` and `update`. A converted file can differ; the source is exact
    /// for plain copies and a safe estimate when conversion shrinks.
    pub bytes_needed: u64,
    /// Bytes given back: removed tracks, and the old copies that updates replace.
    pub bytes_freed: u64,
    pub free_bytes: u64,
    /// Whether the sync fits, leaving [`RESERVE_BYTES`] free.
    pub fits: bool,
    #[serde(skip)]
    pub(crate) manifest_revision: u64,
}

/// Plans a sync of `items` and `playlists` to `device`: what to add, update and remove, and
/// whether it fits. Reads the device and changes nothing.
pub fn plan(device: &Device, items: &[SyncItem], playlists: &[SyncPlaylist]) -> Result<SyncPlan> {
    let storage = storage::open(device)?;
    plan_with(
        Target {
            storage: &*storage,
            profile: &device.profile,
        },
        items,
        playlists,
    )
}

/// [`plan`] for any [`Storage`](crate::Storage), such as an MTP backend's.
pub fn plan_with(
    target: Target<'_>,
    items: &[SyncItem],
    playlists: &[SyncPlaylist],
) -> Result<SyncPlan> {
    let Target { storage, profile } = target;
    profile.validate()?;
    let template = Template::parse(&profile.layout)?;
    let manifest = Manifest::load(storage)?;
    let inventory = Inventory::scan(storage, profile)?;

    let mut wanted: BTreeMap<i64, &SyncItem> = BTreeMap::new();
    for item in items {
        wanted.entry(item.library_id).or_insert(item);
    }
    let removed: Vec<(&i64, &crate::manifest::Entry)> = manifest
        .items
        .iter()
        .filter(|(id, _)| !wanted.contains_key(id))
        .collect();

    // Paths about to be freed (removals, an earlier interrupted sync's trash and orphans) can be
    // reused for new files, because the applier clears them first.
    let owned = manifest.owned_paths();
    let orphans = manifest
        .pending
        .iter()
        .filter(|path| !owned.contains(&path.to_lowercase()));
    let freed = lowercase_set(
        removed
            .iter()
            .map(|(_, entry)| entry.path.as_str())
            .chain(manifest.trash.iter().map(String::as_str))
            .chain(orphans.map(String::as_str)),
    );

    // A file we keep claims its path first, whatever the new files want.
    let mut claimed: HashSet<String> = HashSet::new();
    let mut unchanged = 0usize;
    let mut add = Vec::new();
    let mut update = Vec::new();
    let mut bytes_freed: u64 = removed.iter().map(|(_, entry)| entry.size).sum();
    let mut moving: Vec<(&SyncItem, String)> = Vec::new();
    let mut fresh: Vec<(&SyncItem, String)> = Vec::new();

    for (&id, &item) in &wanted {
        let base = template.path(profile, item, &extension_for(profile, &item.source));
        let Some(entry) = manifest.items.get(&id) else {
            fresh.push((item, base));
            continue;
        };
        if !same_destination(&entry.path, &base) {
            moving.push((item, base));
            continue;
        }
        claimed.insert(entry.path.to_lowercase());
        let stale = entry.source_mtime != item.mtime || entry.source_size != item.size;
        let planned = |bytes| PlannedFile {
            library_id: id,
            path: entry.path.clone(),
            replaces: None,
            bytes,
        };
        match inventory.size(&entry.path) {
            None => add.push(planned(item.size)),
            Some(size) if stale || size != entry.size => {
                bytes_freed += entry.size;
                update.push(planned(item.size));
            }
            Some(_) => unchanged += 1,
        }
    }

    let taken = |candidate: &str| {
        claimed.contains(candidate) || (inventory.contains(candidate) && !freed.contains(candidate))
    };
    // Everything that needs a new path is placed in library-id order, so the same state always
    // gives the same names.
    let mut placements: Vec<(&SyncItem, String, bool)> = moving
        .into_iter()
        .map(|(item, base)| (item, base, true))
        .collect();
    placements.extend(fresh.into_iter().map(|(item, base)| (item, base, false)));
    placements.sort_by_key(|(item, ..)| item.library_id);
    let mut placed: HashSet<String> = HashSet::new();
    for (item, base, is_move) in placements {
        let path = first_free(&base, &|candidate| {
            taken(candidate) || placed.contains(candidate)
        });
        placed.insert(path.to_lowercase());
        let replaces = is_move.then(|| manifest.items[&item.library_id].clone());
        if let Some(old) = &replaces {
            bytes_freed += old.size;
        }
        let file = PlannedFile {
            library_id: item.library_id,
            path,
            replaces: replaces.map(|old| old.path),
            bytes: item.size,
        };
        if is_move {
            update.push(file)
        } else {
            add.push(file)
        }
    }

    add.sort_by(|a, b| a.path.cmp(&b.path));
    update.sort_by(|a, b| a.path.cmp(&b.path));
    let remove: Vec<RemovedFile> = removed
        .iter()
        .map(|(library_id, entry)| RemovedFile {
            library_id: **library_id,
            path: entry.path.clone(),
            bytes: entry.size,
        })
        .collect();

    let wanted_playlists: HashSet<&str> = playlists
        .iter()
        .map(|playlist| playlist.name.as_str())
        .collect();
    let bytes_needed: u64 = add.iter().chain(&update).map(|file| file.bytes).sum();
    let removal_bytes: u64 = remove.iter().map(|file| file.bytes).sum();
    let free_bytes = storage
        .free_bytes()
        .map_err(|source| Error::io("free space", source))?;
    let largest = add
        .iter()
        .chain(&update)
        .map(|file| file.bytes)
        .max()
        .unwrap_or(0);
    // Removals run first, so they count towards the room for the largest file; the rest is the net
    // growth, with the reserve left over.
    let fits = free_bytes.saturating_add(removal_bytes) >= largest.saturating_add(RESERVE_BYTES)
        && bytes_needed
            .saturating_sub(bytes_freed)
            .saturating_add(RESERVE_BYTES)
            <= free_bytes;

    Ok(SyncPlan {
        add,
        update,
        remove,
        unchanged,
        write_playlists: playlists
            .iter()
            .map(|playlist| playlist.name.clone())
            .collect(),
        remove_playlists: manifest
            .playlists
            .keys()
            .filter(|name| !wanted_playlists.contains(name.as_str()))
            .cloned()
            .collect(),
        bytes_needed,
        bytes_freed,
        free_bytes,
        fits,
        manifest_revision: manifest.revision,
    })
}

/// The extension the file on the device gets: the source's own if the device plays it, otherwise
/// the device's preferred format, which `prepare` is expected to convert to.
pub(crate) fn extension_for(profile: &Profile, source: &Path) -> String {
    let own = source
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_lowercase)
        .unwrap_or_default();
    let plays = profile.formats.is_empty()
        || profile
            .formats
            .iter()
            .any(|format| format.eq_ignore_ascii_case(&own));
    match (plays && !own.is_empty(), profile.formats.first()) {
        (true, _) => own,
        (false, Some(preferred)) => preferred.to_lowercase(),
        (false, None) => "bin".to_owned(),
    }
}
