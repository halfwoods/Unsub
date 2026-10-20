use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Local;

use super::commit::{self, Staged};
use super::files::{self, Rng};
use super::mapping::{MappedImage, MappedPlaylist, MappedTrack, Mapping};
use super::on_the_go;
use super::plan::{Space, artwork_bytes, freed_bytes, reserve_bytes, space};
use super::{
    SyncPlan, SyncPlaylist, SyncProgress, SyncStage, SyncTrack, checksum_key, shuffle_format,
};
use crate::bytes::u64_at;
use crate::database::{Database, Usage};
use crate::device::{Ipod, database_path, itunes_dir};
use crate::error::{Error, Result};
use crate::itunesdb::artwork::{self, Artwork, KnownImage, Link, Writer};
use crate::itunesdb::raw::{RawDb, RawPlaylist, RawTrack};
use crate::itunesdb::shuffle::{self, ShuffleFormat};
use crate::itunesdb::{self, Metadata, Origin};

/// iTunes numbers the first track 52 (libgpod's `FIRST_IPOD_ID`).
const FIRST_TRACK_ID: u32 = 52;
/// The mhit header size of iTunes 9, used when the iPod has no track to copy the size from.
const DEFAULT_TRACK_HEADER_LEN: usize = 0x184;
/// Where the mhbd header repeats a field in every mhit (libgpod's `id_0x24`).
const DATABASE_TAG_AT: usize = 0x24;

/// Applies `plan`: copies the new music, rewrites the database and removes what the plan drops.
///
/// `prepare` turns a source file into one the iPod can play and returns the path to copy; it is
/// never deleted by this function. `cover` returns the JPEG or PNG for a library track; it is
/// called only for tracks being added or whose [`SyncTrack::artwork_key`] changed, one at a time
/// (once per album and key), so covers never have to be held in memory together. Returns the
/// database as written.
///
/// Order, chosen so that a failure leaves the iPod usable: all music is copied first and cover
/// thumbnails appended (a failure here removes what was copied and nothing else has changed), then
/// the bookkeeping file, the ArtworkDB and the iTunesDB are each written beside the old one and
/// flushed, the old ones are kept as `*.antistreaming-backup`, and the new ones are renamed into
/// place, the iTunesDB last. Only then are the files of removed tracks deleted.
///
/// If the plan has `removals_first`, there is not room to copy before deleting. Then the removal
/// is committed on its own first (the same way, as a database without those tracks), their files
/// are deleted, and the rest follows. A failure after that point reports
/// [`Error::AfterRemovals`]; the iPod has fewer tracks than before and a valid database.
///
/// The iPod's `Play Counts` file is not touched: call [`super::read_play_counts_for_sync_back`]
/// before this, merge the result into the library, and [`super::clear_play_counts`] after.
pub fn apply_sync(
    ipod: &Ipod,
    plan: &SyncPlan,
    tracks: &[SyncTrack],
    playlists: &[SyncPlaylist],
    prepare: &mut dyn FnMut(&SyncTrack) -> io::Result<PathBuf>,
    cover: &mut dyn FnMut(i64) -> Option<Vec<u8>>,
    progress: &mut dyn FnMut(SyncProgress),
) -> Result<Database> {
    let mut job = Job::start(ipod, plan, tracks, playlists)?;
    let outcome = job.run(prepare, cover, progress);
    if outcome.is_err() {
        job.roll_back();
    }
    outcome
}

/// The database file as it was when the sync began, to notice anyone else changing it meanwhile.
struct Snapshot {
    bytes: Vec<u8>,
    len: u64,
    modified: Option<SystemTime>,
}

impl Snapshot {
    fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).map_err(|source| Error::io(path, source))?;
        let meta = fs::metadata(path).map_err(|source| Error::io(path, source))?;
        Ok(Self {
            bytes,
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }

    fn unchanged(&self, path: &Path) -> bool {
        fs::metadata(path)
            .is_ok_and(|meta| meta.len() == self.len && meta.modified().ok() == self.modified)
    }
}

struct Added {
    library_id: i64,
    dbid: u64,
    track: RawTrack,
}

struct Job<'a> {
    ipod: &'a Ipod,
    mount: &'a Path,
    database: PathBuf,
    checksum: Option<[u8; 8]>,
    snapshot: Snapshot,
    raw: RawDb,
    mapping: Mapping,
    plan: &'a SyncPlan,
    sources: HashMap<i64, &'a SyncTrack>,
    playlists: &'a [SyncPlaylist],
    /// What this iPod can do with album art; `None` if nothing is written for it.
    artwork: Option<Artwork>,
    /// The iTunesSD to keep in step with the iTunesDB, if this iPod is a shuffle.
    shadow: Option<Shadow>,
    /// Whether the tracks being removed must go before anything is copied, to make room.
    removals_first: bool,
    /// The previous databases are kept once per run, at its first commit.
    backups_kept: bool,
    /// Music files created by this run, removed again if the run fails before the commit.
    copied: Vec<PathBuf>,
    /// Thumbnails appended to `.ithmb` files and the ArtworkDB that shows them.
    written_artwork: Option<artwork::Written>,
    /// The image each track that got a cover points at, by the track's persistent id.
    artwork_links: HashMap<u64, Link>,
    /// The ArtworkDB as it was when the run read it, to notice anyone else changing it.
    artwork_stamp: Option<Stamp>,
    /// The iTunesDB that refers to `copied` has been put in place.
    committed: bool,
    /// The ArtworkDB that refers to the appended thumbnails has been put in place.
    artwork_committed: bool,
}

/// The iTunesSD a shuffle plays from, written after the iTunesDB with the same care.
struct Shadow {
    format: ShuffleFormat,
    path: PathBuf,
    /// Carried over from the iTunesSD already on the iPod.
    volume_limit: u8,
    /// The file as it was when the run read it, to notice anyone else changing it.
    stamp: Option<Stamp>,
}

/// Length and modification time, enough to see that another program rewrote a file.
type Stamp = (u64, Option<SystemTime>);

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

impl<'a> Job<'a> {
    /// Reads and checks everything. Nothing on the iPod has been changed when this returns.
    fn start(
        ipod: &'a Ipod,
        plan: &'a SyncPlan,
        tracks: &'a [SyncTrack],
        playlists: &'a [SyncPlaylist],
    ) -> Result<Self> {
        let checksum = checksum_key(ipod)?;
        let mount = ipod.mount_path.as_path();
        probe_writable(mount)?;
        let database = database_path(mount, ipod.database);
        let snapshot = Snapshot::read(&database)?;
        let raw = RawDb::parse(&snapshot.bytes)?;
        let mapping = Mapping::load(mount)?;
        let shadow = shuffle_format(ipod)?.map(|format| {
            let path = itunes_dir(mount).join(shuffle::FILE_NAME);
            let existing = fs::read(&path).unwrap_or_default();
            Shadow {
                format,
                volume_limit: shuffle::volume_limit(&existing),
                stamp: stamp(&path),
                path,
            }
        });
        let mut sources = HashMap::new();
        for track in tracks {
            sources.entry(track.library_id).or_insert(track);
        }
        let mut job = Self {
            ipod,
            mount,
            database,
            checksum,
            snapshot,
            raw,
            mapping,
            plan,
            sources,
            playlists,
            artwork: Artwork::for_device(mount, ipod.model.as_ref()),
            shadow,
            removals_first: false,
            backups_kept: false,
            copied: Vec::new(),
            written_artwork: None,
            artwork_links: HashMap::new(),
            artwork_stamp: None,
            committed: false,
            artwork_committed: false,
        };
        job.validate_plan()?;
        job.removals_first = job.check_space()?;
        Ok(job)
    }

    /// The plan was made from a snapshot of the iPod and the library; refuse it if either has moved
    /// on, and never act on a track this app did not add.
    fn validate_plan(&self) -> Result<()> {
        let conflict =
            |message: String| Err(Error::Conflict(format!("{message}; nothing was written")));
        // Removing a track moves every later one up the list, which the iPod's playlists made with
        // its On-The-Go menu count positions in.
        if !self.plan.remove.is_empty() && on_the_go::present(self.mount) {
            return conflict(
                "the iPod has On-The-Go playlists, which count tracks by position; read them with \
                 read_on_the_go and delete them with clear_on_the_go before removing tracks"
                    .to_owned(),
            );
        }
        let dbid_of = self.mapping.dbid_by_library_id();
        let owned: HashSet<u64> = dbid_of.values().copied().collect();
        let by_id: HashMap<u32, &RawTrack> = self
            .raw
            .tracks
            .iter()
            .map(|track| (track.id(), track))
            .collect();
        let present: HashSet<u64> = self.raw.tracks.iter().map(RawTrack::dbid).collect();

        for id in &self.plan.remove {
            match by_id.get(id) {
                None => return conflict(format!("track {id} is no longer on the iPod")),
                Some(track) if !owned.contains(&track.dbid()) => {
                    return conflict(format!(
                        "track {id} was not added by this app and will not be removed"
                    ));
                }
                Some(_) => {}
            }
        }
        for library_id in &self.plan.update {
            let on_ipod = dbid_of
                .get(library_id)
                .is_some_and(|dbid| present.contains(dbid));
            if !self.sources.contains_key(library_id) || !on_ipod {
                return conflict(format!(
                    "track {library_id} cannot be updated: it is not on the iPod or not in the library"
                ));
            }
        }
        for library_id in &self.plan.add {
            let on_ipod = dbid_of
                .get(library_id)
                .is_some_and(|dbid| present.contains(dbid));
            if !self.sources.contains_key(library_id) || on_ipod {
                return conflict(format!(
                    "track {library_id} cannot be added: it is already on the iPod or not in the library"
                ));
            }
        }
        Ok(())
    }

    /// Whether the new music fits, and if only after deleting the removed tracks first.
    fn check_space(&self) -> Result<bool> {
        let music = self.plan.add.iter().try_fold(0u64, |total, library_id| {
            let source = &self.sources[library_id].source_path;
            fs::metadata(source)
                .map(|meta| total + meta.len())
                .map_err(|error| Error::io(source, error))
        })?;
        let needed = music + artwork_bytes(self.artwork.as_ref(), self.tracks_needing_art());
        let reserve = reserve_bytes(self.mount, self.ipod);
        let freed = freed_bytes(self.removed_files());
        let available =
            fs4::available_space(self.mount).map_err(|source| Error::io(self.mount, source))?;
        match space(needed, reserve, available, freed) {
            Space::Fits => Ok(false),
            Space::AfterRemovals => Ok(true),
            Space::Short { available } => Err(Error::InsufficientSpace {
                needed: needed.saturating_add(reserve),
                available,
            }),
        }
    }

    /// Tracks that will get a cover written: new ones with a cover, and existing ones whose cover
    /// key differs from the one recorded when it was last written.
    fn tracks_needing_art(&self) -> Vec<&'a SyncTrack> {
        if self.artwork.is_none() {
            return Vec::new();
        }
        let recorded = self.mapping.artwork_key_by_library_id();
        let changed = self.plan.update.iter().filter(|library_id| {
            recorded.get(library_id) != Some(&self.sources[library_id].artwork_key)
        });
        self.plan
            .add
            .iter()
            .chain(changed)
            .map(|library_id| self.sources[library_id])
            .filter(|track| track.artwork_key.is_some())
            .collect()
    }

    /// The files of the tracks the plan removes, as the database names them.
    fn removed_files(&self) -> Vec<PathBuf> {
        let remove: HashSet<u32> = self.plan.remove.iter().copied().collect();
        self.raw
            .tracks
            .iter()
            .filter(|track| remove.contains(&track.id()))
            .filter_map(|track| files::music_path(self.mount, track))
            .collect()
    }

    fn require_free(&self, bytes: u64) -> Result<()> {
        let available =
            fs4::available_space(self.mount).map_err(|source| Error::io(self.mount, source))?;
        let needed = bytes.saturating_add(reserve_bytes(self.mount, self.ipod));
        if needed > available {
            return Err(Error::InsufficientSpace { needed, available });
        }
        Ok(())
    }

    fn run(
        &mut self,
        prepare: &mut dyn FnMut(&SyncTrack) -> io::Result<PathBuf>,
        cover: &mut dyn FnMut(i64) -> Option<Vec<u8>>,
        progress: &mut dyn FnMut(SyncProgress),
    ) -> Result<Database> {
        if !self.removals_first {
            return self.finish(prepare, cover, progress);
        }
        let removed = self.plan.remove.len();
        self.remove_first(progress, removed)?;
        self.finish(prepare, cover, progress)
            .map_err(|source| Error::AfterRemovals {
                removed,
                source: Box::new(source),
            })
    }

    /// Commits a database without the removed tracks and deletes their files, to make room. Until
    /// that commit succeeds nothing has changed; after it, a failure is reported as
    /// [`Error::AfterRemovals`].
    fn remove_first(
        &mut self,
        progress: &mut dyn FnMut(SyncProgress),
        removed: usize,
    ) -> Result<()> {
        let removed_files = self.remove_tracks();
        let mapping = Mapping::new(
            self.mapping_tracks_present(),
            self.mapping.playlists.clone(),
            self.mapping.artwork.clone(),
        );
        progress(SyncProgress {
            stage: SyncStage::WritingDatabase,
            done: 0,
            total: 1,
            current: String::new(),
        });
        let bytes = itunesdb::emit(&self.raw, self.checksum.as_ref())?;
        self.self_check(&bytes)?;
        self.commit(&bytes, &mapping)?;
        progress(SyncProgress {
            stage: SyncStage::WritingDatabase,
            done: 1,
            total: 1,
            current: String::new(),
        });
        // Nothing copied by this run is referenced yet, so a later failure must still remove it.
        self.committed = false;
        let after_removals = |source| Error::AfterRemovals {
            removed,
            source: Box::new(source),
        };
        self.snapshot = Snapshot::read(&self.database).map_err(after_removals)?;
        if let Some(shadow) = &mut self.shadow {
            shadow.stamp = stamp(&shadow.path);
        }
        self.mapping = mapping;
        self.delete_removed_files(&removed_files, progress);
        Ok(())
    }

    fn finish(
        &mut self,
        prepare: &mut dyn FnMut(&SyncTrack) -> io::Result<PathBuf>,
        cover: &mut dyn FnMut(i64) -> Option<Vec<u8>>,
        progress: &mut dyn FnMut(SyncProgress),
    ) -> Result<Database> {
        // Which images the iPod's current database uses, before this run's edits.
        let referenced: HashSet<u32> = self
            .raw
            .tracks
            .iter()
            .map(RawTrack::artwork_link)
            .filter(|&link| link != 0)
            .collect();
        let added = self.copy_new_music(prepare, progress)?;
        self.write_artwork(&added, &referenced, cover)?;
        let (removed_files, mapping) = self.edit_database(added);

        progress(SyncProgress {
            stage: SyncStage::WritingDatabase,
            done: 0,
            total: 1,
            current: String::new(),
        });
        let bytes = itunesdb::emit(&self.raw, self.checksum.as_ref())?;
        let written = self.self_check(&bytes)?;
        self.commit(&bytes, &mapping)?;
        progress(SyncProgress {
            stage: SyncStage::WritingDatabase,
            done: 1,
            total: 1,
            current: String::new(),
        });

        self.delete_removed_files(&removed_files, progress);
        Ok(written)
    }

    /// Fetches, renders and appends the thumbnails for covers that need them, one cover at a time.
    fn write_artwork(
        &mut self,
        added: &[Added],
        referenced: &HashSet<u32>,
        cover: &mut dyn FnMut(i64) -> Option<Vec<u8>>,
    ) -> Result<()> {
        let Some(device) = &self.artwork else {
            return Ok(());
        };
        let recorded = self.mapping.artwork_key_by_library_id();
        let dbid_of = self.mapping.dbid_by_library_id();
        let changed = self
            .plan
            .update
            .iter()
            .filter(|library_id| {
                recorded.get(library_id) != Some(&self.sources[library_id].artwork_key)
            })
            .filter_map(|library_id| Some((*library_id, *dbid_of.get(library_id)?)));
        let wanting: Vec<(i64, u64)> = added
            .iter()
            .map(|added| (added.library_id, added.dbid))
            .chain(changed)
            .filter(|(library_id, _)| self.sources[library_id].artwork_key.is_some())
            .collect();
        let known: Vec<KnownImage> = self
            .mapping
            .artwork
            .iter()
            .map(|image| KnownImage {
                key: image.key.clone(),
                image_id: image.image_id,
            })
            .collect();
        if wanting.is_empty() && known.is_empty() {
            return Ok(());
        }
        self.artwork_stamp = stamp(&artwork::database_path(self.mount));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
        let mut writer = Writer::open(device, self.mount, &known, now)?;

        // Where tracks share an image, tracks of one album with one key share a cover, which is
        // then fetched once.
        let mut done: HashMap<(&str, u64), Option<Link>> = HashMap::new();
        for (library_id, dbid) in wanting {
            let source = self.sources[&library_id];
            let Some(key) = source.artwork_key else {
                continue;
            };
            let shared = device
                .shares_images()
                .then(|| done.get(&(source.album.as_str(), key)));
            let link = match shared.flatten() {
                Some(link) => *link,
                None => {
                    let link = match cover(library_id).filter(|data| !data.is_empty()) {
                        Some(data) => writer
                            .add(dbid, &source.album, &data)
                            .inspect_err(|_| writer.roll_back())?,
                        None => None,
                    };
                    if device.shares_images() {
                        done.insert((source.album.as_str(), key), link);
                    }
                    link
                }
            };
            if let Some(link) = link {
                self.artwork_links.insert(dbid, link);
            }
        }
        self.written_artwork = Some(writer.finish(referenced)?);
        Ok(())
    }

    fn copy_new_music(
        &mut self,
        prepare: &mut dyn FnMut(&SyncTrack) -> io::Result<PathBuf>,
        progress: &mut dyn FnMut(SyncProgress),
    ) -> Result<Vec<Added>> {
        let folders = files::music_folders(self.mount)?;
        let mut rng = Rng::new();
        let mut next_id = self
            .raw
            .tracks
            .iter()
            .map(RawTrack::id)
            .max()
            .map_or(FIRST_TRACK_ID, |max| max + 1);
        let mut used_dbids: HashSet<u64> = self.raw.tracks.iter().map(RawTrack::dbid).collect();
        let header_len = self
            .raw
            .tracks
            .first()
            .map_or(DEFAULT_TRACK_HEADER_LEN, |track| track.header.len());
        let database_tag = u64_at(&self.raw.header, DATABASE_TAG_AT).unwrap_or(0);
        let total = self.plan.add.len() as u64;

        let mut added = Vec::new();
        for (done, library_id) in self.plan.add.iter().enumerate() {
            let track = self.sources[library_id];
            let step = |stage| SyncProgress {
                stage,
                done: done as u64,
                total,
                current: track.title.clone(),
            };
            progress(step(SyncStage::Preparing));
            let prepared =
                prepare(track).map_err(|source| Error::io(&track.source_path, source))?;
            let extension = lower_extension(&prepared).ok_or_else(|| {
                Error::io(
                    &prepared,
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "the prepared file has no extension",
                    ),
                )
            })?;
            if !files::is_playable_extension(&extension) {
                let message = format!(
                    "iPods cannot play .{extension} files; convert to MP3, AAC, Apple Lossless, WAV or AIFF"
                );
                return Err(Error::io(
                    &prepared,
                    io::Error::new(io::ErrorKind::InvalidInput, message),
                ));
            }
            let size = fs::metadata(&prepared)
                .map_err(|source| Error::io(&prepared, source))?
                .len();
            self.require_free(size)?;

            progress(step(SyncStage::Copying));
            let placed = files::place(&folders, &extension, &mut rng)
                .map_err(|source| Error::io(files::music_root(self.mount), source))?;
            let destination = placed.path.clone();
            self.copied.push(destination.clone());
            files::copy_into(&prepared, placed)
                .map_err(|source| Error::io(&destination, source))?;

            let dbid = loop {
                let candidate = rng.next();
                if candidate != 0 && used_dbids.insert(candidate) {
                    break candidate;
                }
            };
            let origin = Origin {
                id: next_id,
                dbid,
                ipod_path: ipod_path(self.mount, &destination),
                codec: files::detect_codec(&destination, &extension),
                file_size: size,
                duration_ms: track.duration_ms,
                bitrate_kbps: track.bitrate_kbps,
                sample_rate: track.sample_rate,
                date_added: track.date_added,
                media_kind: track.media_kind,
                database_tag,
                extension,
            };
            next_id += 1;
            added.push(Added {
                library_id: *library_id,
                dbid,
                track: RawTrack::create(header_len, &origin, &metadata(track), &Local),
            });
        }
        Ok(added)
    }

    /// Drops the removed tracks from the in-memory database and the playlists that held them.
    /// Returns their files, to delete once the new database is in place.
    fn remove_tracks(&mut self) -> Vec<PathBuf> {
        let removed_files = self.removed_files();
        let remove: HashSet<u32> = self.plan.remove.iter().copied().collect();
        self.raw
            .tracks
            .retain(|track| !remove.contains(&track.id()));
        for playlist in &mut self.raw.playlists {
            playlist.members.retain(|id| !remove.contains(id));
        }
        removed_files
    }

    /// The bookkeeping entries for tracks still in the database.
    fn mapping_tracks_present(&self) -> Vec<MappedTrack> {
        let present: HashSet<u64> = self.raw.tracks.iter().map(RawTrack::dbid).collect();
        self.mapping
            .tracks
            .iter()
            .filter(|mapped| present.contains(&mapped.dbid))
            .cloned()
            .collect()
    }

    /// Applies the plan to the in-memory database. Returns the files to delete once the commit has
    /// succeeded and the bookkeeping file to commit with it.
    fn edit_database(&mut self, added: Vec<Added>) -> (Vec<PathBuf>, Mapping) {
        let removed_files = self.remove_tracks();

        let dbid_of = self.mapping.dbid_by_library_id();
        let position_of: HashMap<u64, usize> = self
            .raw
            .tracks
            .iter()
            .enumerate()
            .map(|(position, track)| (track.dbid(), position))
            .collect();
        for library_id in &self.plan.update {
            if let Some(&position) = dbid_of
                .get(library_id)
                .and_then(|dbid| position_of.get(dbid))
            {
                let track = &mut self.raw.tracks[position];
                track.apply(&metadata(self.sources[library_id]), &Local);
                if let Some(link) = self.artwork_links.get(&track.dbid()) {
                    track.link_artwork(link.image_id, link.source_len);
                }
            }
        }

        let podcasts_playlist = self.raw.playlists.iter().any(RawPlaylist::is_podcasts);
        let mut master_additions = Vec::new();
        let mut podcast_additions = Vec::new();
        let mut new_mappings = Vec::new();
        for Added {
            library_id,
            dbid,
            mut track,
        } in added
        {
            if let Some(link) = self.artwork_links.get(&dbid) {
                track.link_artwork(link.image_id, link.source_len);
            }
            let id = track.id();
            // Like libgpod, keep podcasts out of the master playlist when they have a playlist of
            // their own; with none they would be unreachable on the iPod.
            if self.sources[&library_id].media_kind == crate::database::IpodMediaKind::Podcast
                && podcasts_playlist
            {
                podcast_additions.push(id);
            } else {
                master_additions.push(id);
            }
            new_mappings.push(MappedTrack {
                library_id,
                dbid,
                artwork_key: self
                    .artwork
                    .as_ref()
                    .and(self.sources[&library_id].artwork_key),
            });
            self.raw.tracks.push(track);
        }
        for playlist in &mut self.raw.playlists {
            if playlist.is_master() {
                playlist.members.extend(&master_additions);
            } else if playlist.is_podcasts() {
                playlist.members.extend(&podcast_additions);
            }
        }

        let mut tracks = self.mapping_tracks_present();
        if self.artwork.is_some() {
            let updated: HashSet<i64> = self.plan.update.iter().copied().collect();
            for mapped in tracks
                .iter_mut()
                .filter(|mapped| updated.contains(&mapped.library_id))
            {
                // Handled now, whether or not a cover turned out to be usable.
                if let Some(key) = self.sources[&mapped.library_id].artwork_key {
                    mapped.artwork_key = Some(key);
                }
            }
        }
        tracks.extend(new_mappings);
        let playlists = self.edit_playlists(&tracks);
        let images = match &self.written_artwork {
            Some(written) => written
                .images
                .iter()
                .map(|image| MappedImage {
                    key: image.key.clone(),
                    image_id: image.image_id,
                })
                .collect(),
            None => self.mapping.artwork.clone(),
        };
        (removed_files, Mapping::new(tracks, playlists, images))
    }

    /// Replaces the playlists this app owns with `self.playlists`; others are not touched.
    fn edit_playlists(&mut self, tracks: &[MappedTrack]) -> Vec<MappedPlaylist> {
        let id_of_dbid: HashMap<u64, u32> = self
            .raw
            .tracks
            .iter()
            .map(|track| (track.dbid(), track.id()))
            .collect();
        let ipod_id: HashMap<i64, u32> = tracks
            .iter()
            .filter_map(|mapped| Some((mapped.library_id, *id_of_dbid.get(&mapped.dbid)?)))
            .collect();
        let owned: HashMap<&str, u64> = self
            .mapping
            .playlists
            .iter()
            .map(|owned| (owned.name.as_str(), owned.id))
            .collect();
        let owned_ids: HashSet<u64> = owned.values().copied().collect();
        self.raw
            .playlists
            .retain(|playlist| !owned_ids.contains(&playlist.id()));

        let mut taken: HashSet<u64> = self.raw.playlists.iter().map(RawPlaylist::id).collect();
        let mut rng = Rng::new();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
        let mut mapped = Vec::new();
        let mut names = HashSet::new();
        for wanted in self
            .playlists
            .iter()
            .filter(|wanted| names.insert(wanted.name.as_str()))
        {
            let members = wanted
                .tracks
                .iter()
                .filter_map(|library_id| ipod_id.get(library_id).copied())
                .collect();
            let id = owned
                .get(wanted.name.as_str())
                .copied()
                .filter(|id| !taken.contains(id))
                .unwrap_or_else(|| {
                    loop {
                        let candidate = rng.next();
                        if candidate != 0
                            && !taken.contains(&candidate)
                            && !owned_ids.contains(&candidate)
                        {
                            break candidate;
                        }
                    }
                });
            taken.insert(id);
            self.raw
                .playlists
                .push(RawPlaylist::create(id, &wanted.name, members, now, &Local));
            mapped.push(MappedPlaylist {
                name: wanted.name.clone(),
                id,
            });
        }
        mapped
    }

    /// Parses what is about to be committed with the same reader that will load it next time, so a
    /// database this crate itself cannot read never reaches the iPod.
    fn self_check(&self, bytes: &[u8]) -> Result<Database> {
        let (tracks, playlists) = itunesdb::parse(bytes, self.mount).map_err(|error| {
            Error::Conflict(format!(
                "the rewritten database failed its own check ({error}); nothing was written"
            ))
        })?;
        let consistent = tracks.len() == self.raw.tracks.len()
            && self.copied.iter().all(|file| file.is_file())
            && playlists.iter().any(|playlist| playlist.is_master);
        if !consistent {
            return Err(Error::Conflict(
                "the rewritten database failed its own check; nothing was written".to_owned(),
            ));
        }
        let usage = Usage::of(&tracks);
        Ok(Database {
            tracks,
            playlists,
            usage,
        })
    }

    /// Puts the bookkeeping file, the ArtworkDB (if it changed), the iTunesDB and, on a shuffle, the
    /// iTunesSD in place, in that order. Each is written and flushed beside its old version first,
    /// so nothing is replaced until all of them exist; each replacement is one atomic rename. The
    /// iTunesDB points at the earlier files: should anything fail between renames, the old iTunesDB
    /// still works with the extra entries (bookkeeping for tracks not yet there, images nobody
    /// points at yet). The iTunesSD goes after it, so a failure there leaves a new iTunesDB beside
    /// an iTunesSD that still lists only files that exist: removed files are deleted only after
    /// this returns.
    fn commit(&mut self, bytes: &[u8], mapping: &Mapping) -> Result<()> {
        let artwork_path = artwork::database_path(self.mount);
        let artwork_bytes = self
            .written_artwork
            .as_ref()
            .and_then(|written| written.database.as_deref());
        let artwork_changes = artwork_bytes.is_some();
        let shuffle_bytes = self
            .shadow
            .as_ref()
            .map(|shadow| shuffle::emit(&self.raw, shadow.format, shadow.volume_limit))
            .transpose()?;
        let mut staged: Vec<(Staged, Kind)> = Vec::new();
        let staging = (|| -> Result<()> {
            staged.push((
                Staged::write(&Mapping::path(self.mount), &mapping.to_json()?)?,
                Kind::Mapping,
            ));
            if let Some(art) = artwork_bytes {
                staged.push((Staged::write(&artwork_path, art)?, Kind::Artwork));
            }
            staged.push((Staged::write(&self.database, bytes)?, Kind::Database));
            if let (Some(shadow), Some(file)) = (&self.shadow, &shuffle_bytes) {
                staged.push((Staged::write(&shadow.path, file)?, Kind::Shadow));
            }
            Ok(())
        })();
        let ready = staging.and_then(|()| self.keep_backups(artwork_changes, &artwork_path));
        if let Err(error) = ready {
            staged.into_iter().for_each(|(file, _)| file.discard());
            return Err(error);
        }

        let mut remaining = staged.into_iter();
        while let Some((file, kind)) = remaining.next() {
            if let Err(error) = file.commit() {
                remaining.for_each(|(file, _)| file.discard());
                return Err(error);
            }
            match kind {
                Kind::Mapping => {}
                Kind::Artwork => self.artwork_committed = true,
                Kind::Database => self.committed = true,
                Kind::Shadow => {}
            }
        }
        Ok(())
    }

    /// Refuses to go on if another program changed the files meanwhile, then copies the current
    /// ones aside (once per run).
    fn keep_backups(&mut self, artwork_changes: bool, artwork_path: &Path) -> Result<()> {
        let conflict = |what: &str| {
            Err(Error::Conflict(format!(
                "the iPod's {what} changed while syncing; nothing was written"
            )))
        };
        if !self.snapshot.unchanged(&self.database) {
            return conflict("database");
        }
        if artwork_changes && stamp(artwork_path) != self.artwork_stamp {
            return conflict("ArtworkDB");
        }
        if let Some(shadow) = &self.shadow
            && stamp(&shadow.path) != shadow.stamp
        {
            return conflict("iTunesSD");
        }
        if !self.backups_kept {
            commit::keep_backups(&self.database)?;
            if let Some(shadow) = &self.shadow
                && shadow.path.exists()
            {
                commit::keep_backups(&shadow.path)?;
            }
            self.backups_kept = true;
        }
        if artwork_changes && artwork_path.exists() {
            commit::keep_backups(artwork_path)?;
        }
        Ok(())
    }

    /// After the commit, so a failed sync never loses music. Failures are ignored: the database no
    /// longer refers to these files, so a leftover costs space but not correctness.
    fn delete_removed_files(&self, files: &[PathBuf], progress: &mut dyn FnMut(SyncProgress)) {
        let still_used: HashSet<PathBuf> = self
            .raw
            .tracks
            .iter()
            .filter_map(|track| files::music_path(self.mount, track))
            .collect();
        let total = files.len() as u64;
        for (done, file) in files.iter().enumerate() {
            progress(SyncProgress {
                stage: SyncStage::Cleaning,
                done: done as u64,
                total,
                current: file.display().to_string(),
            });
            if !still_used.contains(file) {
                files::remove_music_file(self.mount, file);
            }
        }
    }

    /// Undoes what a failed run left that no committed database refers to: copied music, and
    /// thumbnails appended to files whose ArtworkDB was not replaced.
    fn roll_back(&self) {
        if !self.committed {
            for file in &self.copied {
                let _ = fs::remove_file(file);
            }
        }
        if !self.artwork_committed
            && let Some(written) = &self.written_artwork
        {
            written.roll_back();
        }
    }
}

enum Kind {
    Mapping,
    Artwork,
    Database,
    Shadow,
}

fn metadata(track: &SyncTrack) -> Metadata {
    Metadata {
        title: track.title.clone(),
        artist: track.artist.clone(),
        album_artist: track.album_artist.clone(),
        album: track.album.clone(),
        genre: track.genre.clone(),
        composer: track.composer.clone(),
        sort_title: track.sort_title.clone(),
        sort_artist: track.sort_artist.clone(),
        sort_album: track.sort_album.clone(),
        sort_album_artist: track.sort_album_artist.clone(),
        year: track.year,
        track_number: track.track_number,
        track_count: track.track_count,
        disc_number: track.disc_number,
        disc_count: track.disc_count,
        rating: track.rating.min(100),
        compilation: track.compilation,
        sound_check: track.sound_check.unwrap_or(0),
        play_count: track.play_count,
        last_played: track.last_played,
    }
}

fn lower_extension(path: &Path) -> Option<String> {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)?
        .to_ascii_lowercase();
    (!extension.is_empty()).then_some(extension)
}

/// `/Volumes/IPOD/iPod_Control/Music/F00/ABCD.mp3` as `:iPod_Control:Music:F00:ABCD.mp3`.
fn ipod_path(mount: &Path, file: &Path) -> String {
    let relative = file.strip_prefix(mount).unwrap_or(file);
    relative.components().fold(String::new(), |path, part| {
        format!("{path}:{}", part.as_os_str().to_string_lossy())
    })
}

/// Fails early, before any music is copied, if the iPod is mounted read-only (HFS+ iPods usually
/// are when connected to Windows or Linux).
pub(super) fn probe_writable(mount: &Path) -> Result<()> {
    let probe = itunes_dir(mount).join(".antistreaming-write-probe");
    File::create(&probe)
        .and_then(|_| fs::remove_file(&probe))
        .map_err(|source| {
            Error::SyncRefused(format!(
                "the iPod cannot be written to ({source}); is it mounted read-only?"
            ))
        })
}
