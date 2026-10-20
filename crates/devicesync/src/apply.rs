//! Carrying out a plan, in an order that a pulled cable cannot turn into a broken or ownerless
//! state.
//!
//! The manifest is the record of which files are ours, so it must never claim a file that is not
//! there or leave one of ours unclaimed. Files are therefore copied to a temporary name and renamed
//! into place; before each batch the manifest lists the paths the batch may create (`pending`), and
//! after it the manifest lists what was created. A sync that dies in between leaves, at worst,
//! files named in `pending`, which the next sync recognises as its own and deletes. Files that are
//! to go are listed in `trash` first, for the same reason. Nothing the manifest does not list is
//! ever deleted.

use std::{
    collections::HashMap,
    fs, io,
    ops::ControlFlow,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::Serialize;

use crate::{
    Device, Error, Profile, Result, Storage, SyncItem, SyncPlan, SyncPlaylist,
    inventory::Inventory,
    layout::{first_free, playlist_name, split_extension},
    manifest::{Entry, Manifest},
    plan::PlannedFile,
    playlist,
    storage::{self, Target},
};

/// The manifest is rewritten after this many files or bytes, whichever comes first: often enough
/// that little is lost to a pulled cable, rarely enough that a big library does not rewrite a
/// megabyte of JSON thousands of times.
const BATCH_FILES: usize = 100;
const BATCH_BYTES: u64 = 1 << 30;
/// Room a single file needs beyond its own size when it is about to be copied.
const MIN_FREE_BYTES: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncStage {
    /// Deleting what the plan drops.
    Cleaning,
    Copying,
    /// A track that could not be prepared (`current` says why) and was left out.
    Skipped,
    Playlists,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub stage: SyncStage,
    pub done: u64,
    pub total: u64,
    pub current: String,
}

/// What a finished sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    /// Tracks copied to the device, new and updated.
    pub copied: usize,
    /// Tracks deleted from the device.
    pub removed: usize,
    /// Tracks `prepare` could not produce a file for, with its reason. They are not on the device,
    /// and the next sync tries them again.
    pub skipped: Vec<(i64, String)>,
}

type Prepare<'a> = &'a mut dyn FnMut(&SyncItem, &Profile) -> io::Result<std::path::PathBuf>;
type Progress<'a> = &'a mut dyn FnMut(SyncProgress);

/// Applies `plan` to `device`: deletes what it drops, copies what it adds and updates, and writes
/// the playlists.
///
/// `prepare` turns a source file into one the device can play and returns the path to copy (the
/// source itself when it already plays); it is never deleted by this function. A track `prepare`
/// fails on is skipped and reported through `progress` as [`SyncStage::Skipped`]; the sync goes on.
/// Any error from the device itself, a full disk or a pulled cable, stops the sync. Either way, and
/// on cancel, the manifest records exactly what was copied, so the next sync carries on from there.
///
/// Refuses, writing nothing, a plan that does not fit or that was made against a different state of
/// the device.
///
/// Returns what was done. Playlists are not counted: they are rewritten every time.
pub fn apply(
    device: &Device,
    plan: &SyncPlan,
    items: &[SyncItem],
    playlists: &[SyncPlaylist],
    prepare: Prepare<'_>,
    progress: Progress<'_>,
    cancel: &AtomicBool,
) -> Result<SyncSummary> {
    let storage = storage::open(device)?;
    apply_with(
        Target {
            storage: &*storage,
            profile: &device.profile,
        },
        plan,
        items,
        playlists,
        prepare,
        progress,
        cancel,
    )
}

/// [`apply`] for any [`Storage`], such as an MTP backend's.
pub fn apply_with(
    target: Target<'_>,
    plan: &SyncPlan,
    items: &[SyncItem],
    playlists: &[SyncPlaylist],
    prepare: Prepare<'_>,
    progress: Progress<'_>,
    cancel: &AtomicBool,
) -> Result<SyncSummary> {
    let mut job = Job::start(target, plan, items, playlists, prepare, progress, cancel)?;
    let outcome = job.run();
    if outcome.is_err() {
        // Record what did get copied. If the device is gone this fails too, and the pending list
        // from before the batch is what the next sync recovers from.
        let _ = job.commit_batch();
    }
    outcome.map(|()| job.summary)
}

struct Job<'a> {
    storage: &'a dyn Storage,
    profile: &'a Profile,
    plan: &'a SyncPlan,
    items: HashMap<i64, &'a SyncItem>,
    playlists: &'a [SyncPlaylist],
    manifest: Manifest,
    inventory: Inventory,
    prepare: Prepare<'a>,
    progress: Progress<'a>,
    cancel: &'a AtomicBool,
    /// Items or playlists changed since the manifest was last saved.
    dirty: bool,
    /// Files replaced by a renamed copy in this batch, to delete once the batch is recorded.
    replaced: Vec<String>,
    summary: SyncSummary,
}

impl<'a> Job<'a> {
    /// Checks everything and cleans up after any interrupted sync. Nothing of the user's has been
    /// changed when this returns.
    fn start(
        target: Target<'a>,
        plan: &'a SyncPlan,
        items: &'a [SyncItem],
        playlists: &'a [SyncPlaylist],
        prepare: Prepare<'a>,
        progress: Progress<'a>,
        cancel: &'a AtomicBool,
    ) -> Result<Self> {
        let Target { storage, profile } = target;
        profile.validate()?;
        let manifest = Manifest::load(storage)?;
        if manifest.revision != plan.manifest_revision {
            return Err(Error::Conflict(
                "the device has changed since this sync was planned; plan it again".to_owned(),
            ));
        }
        if !plan.fits {
            return Err(Error::NotEnoughSpace {
                needed: plan.bytes_needed,
                available: plan.free_bytes,
            });
        }
        let mut by_id = HashMap::new();
        for item in items {
            by_id.entry(item.library_id).or_insert(item);
        }
        let inventory = Inventory::scan(storage, profile)?;
        let mut job = Self {
            storage,
            profile,
            plan,
            items: by_id,
            playlists,
            manifest,
            inventory,
            prepare,
            progress,
            cancel,
            dirty: false,
            replaced: Vec::new(),
            summary: SyncSummary::default(),
        };
        job.validate_plan()?;
        job.recover()?;
        Ok(job)
    }

    /// The plan is data from outside, so it is checked rather than trusted: it may only write below
    /// the music folder, only replace or delete what the manifest lists for that very track, and
    /// only refer to tracks it was given.
    fn validate_plan(&self) -> Result<()> {
        let music = format!("{}/", self.profile.music_dir());
        let owners = self.manifest.track_owners();
        for file in self.plan.add.iter().chain(&self.plan.update) {
            let owner = owners.get(&file.path.to_lowercase()).copied();
            let sound = file.path.starts_with(&music)
                && !file.path.contains("..")
                && self.items.contains_key(&file.library_id)
                && owner.is_none_or(|id| id == file.library_id)
                && file.replaces.as_ref().is_none_or(|old| {
                    self.manifest
                        .items
                        .get(&file.library_id)
                        .is_some_and(|entry| &entry.path == old)
                });
            if !sound {
                return Err(Error::Conflict(format!(
                    "the plan does not match the device: {}",
                    file.path
                )));
            }
        }
        for file in &self.plan.remove {
            if self
                .manifest
                .items
                .get(&file.library_id)
                .is_none_or(|entry| entry.path != file.path)
            {
                return Err(Error::Conflict(format!(
                    "{} is not a file this app put on the device",
                    file.path
                )));
            }
        }
        Ok(())
    }

    /// Finishes what an interrupted sync started: deletes its half-written files, the files it was
    /// in the middle of deleting, and the ones it may have created without recording.
    fn recover(&mut self) -> Result<()> {
        let owned = self.manifest.owned_paths();
        let stale: Vec<String> = self
            .inventory
            .leftovers()
            .iter()
            .cloned()
            .chain(self.manifest.trash.iter().cloned())
            .chain(self.manifest.pending.iter().cloned())
            .filter(|path| !owned.contains(&path.to_lowercase()))
            .collect();
        if stale.is_empty() && self.manifest.pending.is_empty() && self.manifest.trash.is_empty() {
            return Ok(());
        }
        for path in &stale {
            self.delete(path)?;
        }
        self.manifest.pending.clear();
        self.manifest.trash.clear();
        self.save()?;
        self.inventory = Inventory::scan(self.storage, self.profile)?;
        Ok(())
    }

    fn run(&mut self) -> Result<()> {
        self.remove_tracks()?;
        self.copy_tracks()?;
        self.write_playlists()?;
        self.commit_batch()
    }

    fn remove_tracks(&mut self) -> Result<()> {
        let total = self.plan.remove.len() as u64;
        let mut paths = Vec::new();
        for (done, file) in (1..).zip(&self.plan.remove) {
            self.manifest.items.remove(&file.library_id);
            self.dirty = true;
            paths.push(file.path.clone());
            (self.progress)(SyncProgress {
                stage: SyncStage::Cleaning,
                done,
                total,
                current: file.path.clone(),
            });
        }
        self.retire(paths)?;
        self.summary.removed = self.plan.remove.len();
        Ok(())
    }

    /// Updates first, then additions, in batches. Before a batch the manifest says which paths it
    /// may create.
    fn copy_tracks(&mut self) -> Result<()> {
        let plan = self.plan;
        let work: Vec<&PlannedFile> = plan.update.iter().chain(&plan.add).collect();
        let total = work.len() as u64;
        let mut done = 0u64;
        let mut rest = work.as_slice();
        while !rest.is_empty() {
            let mut count = 0;
            let mut bytes = 0u64;
            while count < rest.len() && count < BATCH_FILES && (count == 0 || bytes < BATCH_BYTES) {
                bytes += rest[count].bytes;
                count += 1;
            }
            let (batch, tail) = rest.split_at(count);
            rest = tail;

            self.manifest.pending = batch.iter().map(|file| file.path.clone()).collect();
            self.save()?;
            for file in batch {
                if self.cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                self.copy_one(file, done, total)?;
                done += 1;
            }
            self.commit_batch()?;
        }
        Ok(())
    }

    fn copy_one(&mut self, file: &PlannedFile, done: u64, total: u64) -> Result<()> {
        let item = self.items[&file.library_id];
        (self.progress)(SyncProgress {
            stage: SyncStage::Copying,
            done,
            total,
            current: item.title.clone(),
        });

        let skip = |job: &mut Self, why: String| {
            (job.progress)(SyncProgress {
                stage: SyncStage::Skipped,
                done,
                total,
                current: format!("{}: {why}", item.title),
            });
            job.summary.skipped.push((item.library_id, why));
        };
        let prepared = match (self.prepare)(item, self.profile) {
            Ok(path) => path,
            Err(error) => {
                skip(self, error.to_string());
                return Ok(());
            }
        };
        let size = match fs::metadata(&prepared) {
            Ok(meta) if meta.is_file() => meta.len(),
            Ok(_) => {
                skip(self, "the prepared file is not a file".to_owned());
                return Ok(());
            }
            Err(error) => {
                skip(self, error.to_string());
                return Ok(());
            }
        };

        let available = self
            .storage
            .free_bytes()
            .map_err(|source| Error::io("free space", source))?;
        if available < size.saturating_add(MIN_FREE_BYTES) {
            return Err(Error::NotEnoughSpace {
                needed: size,
                available,
            });
        }
        let dest = self.destination(file, &prepared)?;
        let cancel = self.cancel;
        let written = match self.storage.put(&prepared, &dest, &mut |_| {
            if cancel.load(Ordering::Relaxed) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }) {
            Ok(written) => written,
            Err(error)
                if error.kind() == io::ErrorKind::Interrupted && cancel.load(Ordering::Relaxed) =>
            {
                return Err(Error::Cancelled);
            }
            Err(error) => return Err(Error::io(&dest, error)),
        };

        self.manifest.items.insert(
            file.library_id,
            Entry {
                path: dest,
                size: written,
                source_size: item.size,
                source_mtime: item.mtime,
            },
        );
        self.dirty = true;
        self.replaced.extend(file.replaces.clone());
        self.summary.copied += 1;
        Ok(())
    }

    /// The planned path, unless `prepare` returned a file of another type than the plan assumed:
    /// then the extension follows the file, and the new name must not land on anything else.
    fn destination(&mut self, file: &PlannedFile, prepared: &Path) -> Result<String> {
        let actual = prepared
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        let (stem, planned) = split_extension(&file.path);
        if actual.is_empty() || actual == planned.to_lowercase() {
            return Ok(file.path.clone());
        }
        let dest = first_free(&format!("{stem}.{actual}"), &|candidate| {
            self.inventory.contains(candidate)
                || self.manifest.owns(candidate)
                || self
                    .manifest
                    .pending
                    .iter()
                    .any(|pending| pending.to_lowercase() == candidate)
        });
        // The recorded intent has to cover the path actually written.
        self.manifest.pending.push(dest.clone());
        self.save()?;
        Ok(dest)
    }

    /// Records the batch: what was created, and what it replaced. Only then are replaced files
    /// deleted.
    fn commit_batch(&mut self) -> Result<()> {
        self.manifest.pending.clear();
        let replaced = std::mem::take(&mut self.replaced);
        self.retire(replaced)
    }

    /// Lists `paths` as to be deleted, saves, then deletes them. Saving first means a sync cut off
    /// between the two still finishes the deletion later, and never leaves the manifest claiming a
    /// file that is gone.
    fn retire(&mut self, paths: Vec<String>) -> Result<()> {
        self.manifest.trash.extend(paths);
        self.save()?;
        if self.manifest.trash.is_empty() {
            return Ok(());
        }
        let owned = self.manifest.owned_paths();
        for path in std::mem::take(&mut self.manifest.trash) {
            if !owned.contains(&path.to_lowercase()) {
                self.delete(&path)?;
            }
        }
        self.save()
    }

    /// Deletes one of our files and the folders it leaves empty, never the music folder itself.
    fn delete(&self, path: &str) -> Result<()> {
        self.storage
            .remove(path)
            .map_err(|source| Error::io(path, source))?;
        let music = self.profile.music_dir();
        let mut dir = path.rsplit_once('/').map(|(dir, _)| dir);
        while let Some(current) =
            dir.filter(|dir| dir.len() > music.len() && dir.starts_with(&format!("{music}/")))
        {
            self.storage.remove_dir_if_empty(current);
            dir = current.rsplit_once('/').map(|(parent, _)| parent);
        }
        Ok(())
    }

    fn save(&mut self) -> Result<()> {
        let changed = std::mem::take(&mut self.dirty);
        self.manifest.save(self.storage, changed)
    }

    /// Writes every playlist from the tracks that are on the device, and deletes the playlists this
    /// app wrote earlier that are no longer wanted. A playlist file the user made is never
    /// overwritten: a clash gets a numbered name.
    fn write_playlists(&mut self) -> Result<()> {
        let dir = self.profile.playlist_dir();
        let extension = self.profile.playlist_format.extension();
        let mut old_files = Vec::new();
        for name in &self.plan.remove_playlists {
            old_files.extend(self.manifest.playlists.remove(name));
            self.dirty = true;
        }

        let on_device: HashMap<i64, &str> = self
            .manifest
            .items
            .iter()
            .map(|(id, entry)| (*id, entry.path.as_str()))
            .collect();
        let mut rendered = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for playlist in self
            .playlists
            .iter()
            .filter(|playlist| seen.insert(playlist.name.as_str()))
        {
            let lines = playlist::lines(&playlist.items, &on_device, &self.items);
            rendered.push((
                playlist.name.clone(),
                playlist::render(self.profile.playlist_format, &dir, &lines),
            ));
        }
        drop(on_device);

        let total = rendered.len() as u64;
        for (done, (name, bytes)) in (1..).zip(rendered) {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let known = self.manifest.playlists.get(&name).cloned();
            // A playlist written in another format earlier moves to the new format's file.
            let path = match known
                .filter(|path| split_extension(path).1.eq_ignore_ascii_case(extension))
            {
                Some(path) => path,
                None => {
                    old_files.extend(self.manifest.playlists.get(&name).cloned());
                    first_free(
                        &format!("{dir}/{}.{extension}", playlist_name(&name)),
                        &|candidate| {
                            self.inventory.contains(candidate) || self.manifest.owns(candidate)
                        },
                    )
                }
            };
            self.manifest.pending = vec![path.clone()];
            self.save()?;
            self.storage
                .write(&path, &bytes)
                .map_err(|source| Error::io(&path, source))?;
            self.manifest.playlists.insert(name.clone(), path);
            self.dirty = true;
            (self.progress)(SyncProgress {
                stage: SyncStage::Playlists,
                done,
                total,
                current: name,
            });
        }
        self.manifest.pending.clear();
        self.retire(old_files)
    }
}
