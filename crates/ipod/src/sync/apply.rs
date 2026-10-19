use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::commit::{self, Staged};
use super::files::{self, Rng};
use super::mapping::{MappedPlaylist, MappedTrack, Mapping};
use super::plan::reserve_bytes;
use super::{SyncPlan, SyncPlaylist, SyncProgress, SyncStage, SyncTrack, checksum_key};
use crate::bytes::u64_at;
use crate::database::{Database, Usage};
use crate::device::{Ipod, database_path, itunes_dir};
use crate::error::{Error, Result};
use crate::itunesdb::raw::{RawDb, RawPlaylist, RawTrack};
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
/// never deleted by this function. Returns the database as written.
///
/// Order, chosen so that a failure leaves the iPod usable: all music is copied first (a failure
/// here removes what was copied and nothing else has changed), then the new database is written
/// beside the old one and flushed, the old one is kept as `iTunesDB.antistreaming-backup`, and the
/// new one is renamed into place. Only then are the files of removed tracks deleted.
///
/// The iPod's `Play Counts` file is not touched: call [`super::read_play_counts_for_sync_back`]
/// before this, merge the result into the library, and [`super::clear_play_counts`] after.
pub fn apply_sync(
    ipod: &Ipod,
    plan: &SyncPlan,
    tracks: &[SyncTrack],
    playlists: &[SyncPlaylist],
    prepare: &mut dyn FnMut(&SyncTrack) -> io::Result<PathBuf>,
    progress: &mut dyn FnMut(SyncProgress),
) -> Result<Database> {
    let mut job = Job::start(ipod, plan, tracks, playlists)?;
    let outcome = job.run(prepare, progress);
    if outcome.is_err() && !job.committed {
        job.remove_copied_files();
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
    /// Music files created by this run, removed again if the run fails before the commit.
    copied: Vec<PathBuf>,
    committed: bool,
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
        let mut sources = HashMap::new();
        for track in tracks {
            sources.entry(track.library_id).or_insert(track);
        }
        let job = Self {
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
            copied: Vec::new(),
            committed: false,
        };
        job.validate_plan()?;
        job.check_space()?;
        Ok(job)
    }

    /// The plan was made from a snapshot of the iPod and the library; refuse it if either has moved
    /// on, and never act on a track this app did not add.
    fn validate_plan(&self) -> Result<()> {
        let conflict =
            |message: String| Err(Error::Conflict(format!("{message}; nothing was written")));
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

    fn check_space(&self) -> Result<()> {
        let needed = self.plan.add.iter().try_fold(0u64, |total, library_id| {
            let source = &self.sources[library_id].source_path;
            fs::metadata(source)
                .map(|meta| total + meta.len())
                .map_err(|error| Error::io(source, error))
        })?;
        self.require_free(needed)
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
        progress: &mut dyn FnMut(SyncProgress),
    ) -> Result<Database> {
        let added = self.copy_new_music(prepare, progress)?;
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
                track: RawTrack::create(header_len, &origin, &metadata(track)),
            });
        }
        Ok(added)
    }

    /// Applies the plan to the in-memory database. Returns the files to delete once the commit has
    /// succeeded and the bookkeeping file to commit with it.
    fn edit_database(&mut self, added: Vec<Added>) -> (Vec<PathBuf>, Mapping) {
        let remove: HashSet<u32> = self.plan.remove.iter().copied().collect();
        let removed_files: Vec<PathBuf> = self
            .raw
            .tracks
            .iter()
            .filter(|track| remove.contains(&track.id()))
            .filter_map(|track| track.text(crate::itunesdb::PATH).ok().flatten())
            .map(|path| itunesdb::native_path(self.mount, &path))
            .collect();
        self.raw
            .tracks
            .retain(|track| !remove.contains(&track.id()));
        for playlist in &mut self.raw.playlists {
            playlist.members.retain(|id| !remove.contains(id));
        }

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
                self.raw.tracks[position].apply(&metadata(self.sources[library_id]));
            }
        }

        let podcasts_playlist = self.raw.playlists.iter().any(RawPlaylist::is_podcasts);
        let mut master_additions = Vec::new();
        let mut podcast_additions = Vec::new();
        let mut new_mappings = Vec::new();
        for Added {
            library_id,
            dbid,
            track,
        } in added
        {
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
            new_mappings.push(MappedTrack { library_id, dbid });
            self.raw.tracks.push(track);
        }
        for playlist in &mut self.raw.playlists {
            if playlist.is_master() {
                playlist.members.extend(&master_additions);
            } else if playlist.is_podcasts() {
                playlist.members.extend(&podcast_additions);
            }
        }

        let present: HashSet<u64> = self.raw.tracks.iter().map(RawTrack::dbid).collect();
        let mut tracks: Vec<MappedTrack> = self
            .mapping
            .tracks
            .iter()
            .filter(|mapped| present.contains(&mapped.dbid))
            .cloned()
            .collect();
        tracks.extend(new_mappings);
        let playlists = self.edit_playlists(&tracks);
        (removed_files, Mapping::new(tracks, playlists))
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
                .push(RawPlaylist::create(id, &wanted.name, members, now));
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

    fn commit(&mut self, bytes: &[u8], mapping: &Mapping) -> Result<()> {
        let mapping_file = Staged::write(&Mapping::path(self.mount), &mapping.to_json()?)?;
        let database_file = match Staged::write(&self.database, bytes) {
            Ok(staged) => staged,
            Err(error) => {
                mapping_file.discard();
                return Err(error);
            }
        };
        let ready = if self.snapshot.unchanged(&self.database) {
            commit::keep_backups(&self.database)
        } else {
            Err(Error::Conflict(
                "the iPod's database changed while syncing; nothing was written".to_owned(),
            ))
        };
        if let Err(error) = ready {
            database_file.discard();
            mapping_file.discard();
            return Err(error);
        }
        if let Err(error) = database_file.commit() {
            mapping_file.discard();
            return Err(error);
        }
        self.committed = true;
        mapping_file.commit()
    }

    /// After the commit, so a failed sync never loses music. Failures are ignored: the database no
    /// longer refers to these files, so a leftover costs space but not correctness.
    fn delete_removed_files(&self, files: &[PathBuf], progress: &mut dyn FnMut(SyncProgress)) {
        let still_used: HashSet<PathBuf> = self
            .raw
            .tracks
            .iter()
            .filter_map(|track| track.text(crate::itunesdb::PATH).ok().flatten())
            .map(|path| itunesdb::native_path(self.mount, &path))
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

    fn remove_copied_files(&self) {
        for file in &self.copied {
            let _ = fs::remove_file(file);
        }
    }
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
fn probe_writable(mount: &Path) -> Result<()> {
    let probe = itunes_dir(mount).join(".antistreaming-write-probe");
    File::create(&probe)
        .and_then(|_| fs::remove_file(&probe))
        .map_err(|source| {
            Error::SyncRefused(format!(
                "the iPod cannot be written to ({source}); is it mounted read-only?"
            ))
        })
}
