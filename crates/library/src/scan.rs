use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, params};
use walkdir::{DirEntry, WalkDir};

use crate::cloud::placeholder_meta;
use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{ScanProgress, ScanSummary, Track, TrackOverrides};
use crate::store::{WriteMode, write_tracks};
use crate::tags::{self, TAG_PARSER_VERSION, TrackMeta};

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "m4b", "mp4", "aac", "flac", "ogg", "oga", "opus", "wav", "aif", "aiff", "wv",
    "ape",
];
const BATCH_SIZE: usize = 500;
const PROGRESS_INTERVAL: u64 = 50;

/// What the library already holds about a file, to decide if it needs reading.
struct Stored {
    file_size: u64,
    date_modified: i64,
    parser_version: u32,
    overrides: Option<String>,
    in_cloud: bool,
    missing: bool,
}

enum Outcome {
    Unchanged,
    Added(Track),
    Updated(Track),
}

impl Library {
    /// Failures on individual files are collected in the summary instead of
    /// aborting the scan; only database errors abort.
    pub fn add_paths(
        &mut self,
        paths: &[PathBuf],
        mut progress: impl FnMut(ScanProgress),
    ) -> Result<ScanSummary> {
        let now = unix_secs(SystemTime::now());
        let mut summary = ScanSummary::default();
        let mut seen = HashSet::new();
        let mut batch = Vec::with_capacity(BATCH_SIZE);
        let media = self.media_settings()?;

        for root in paths {
            // The folder the user chose, which a placeholder's artist and album are read relative to.
            let scanned_root = fs::canonicalize(root).ok();
            // Following links lets a symlinked music folder work; walkdir
            // reports cycles as errors instead of looping.
            let walker = WalkDir::new(root)
                .follow_links(true)
                .into_iter()
                .filter_entry(|entry| entry.depth() == 0 || !is_hidden(entry));
            for entry in walker {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(err) => {
                        let path = err
                            .path()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        summary.failed.push((path, err.to_string()));
                        continue;
                    }
                };
                if !entry.file_type().is_file() || !is_audio(entry.path()) {
                    continue;
                }
                let canonical = match fs::canonicalize(entry.path()) {
                    Ok(canonical) => canonical,
                    Err(err) => {
                        summary.scanned += 1;
                        summary
                            .failed
                            .push((entry.path().display().to_string(), err.to_string()));
                        continue;
                    }
                };
                // Overlapping roots would otherwise count the same file twice.
                if !seen.insert(canonical.clone()) {
                    continue;
                }

                summary.scanned += 1;
                match self.examine(&canonical, scanned_root.as_deref(), now) {
                    Ok(Outcome::Unchanged) => summary.unchanged += 1,
                    Ok(Outcome::Added(mut track)) => {
                        match self.place_new_file(&mut track, &media) {
                            Ok(()) => {
                                summary.added += 1;
                                batch.push(track);
                            }
                            Err(err) => summary
                                .failed
                                .push((canonical.display().to_string(), err.to_string())),
                        }
                    }
                    Ok(Outcome::Updated(track)) => {
                        summary.updated += 1;
                        batch.push(track);
                    }
                    Err(err) => summary
                        .failed
                        .push((canonical.display().to_string(), err.to_string())),
                }

                if batch.len() >= BATCH_SIZE {
                    self.flush(&mut batch)?;
                }
                if summary.scanned % PROGRESS_INTERVAL == 0 {
                    progress(ScanProgress {
                        scanned: summary.scanned,
                        added: summary.added,
                        current_path: canonical.display().to_string(),
                    });
                }
            }
        }
        self.flush(&mut batch)?;
        Ok(summary)
    }

    fn examine(&self, canonical: &Path, root: Option<&Path>, now: i64) -> Result<Outcome> {
        let facts = FileFacts::of(canonical)?;
        let dataless = (self.is_dataless)(canonical);
        let stored: Option<Stored> = self
            .conn
            .prepare_cached(
                "SELECT file_size, date_modified, parser_version, overrides, in_cloud, missing
                 FROM tracks WHERE path = ?1",
            )?
            .query_row([&facts.path], |row| {
                Ok(Stored {
                    file_size: row.get(0)?,
                    date_modified: row.get(1)?,
                    parser_version: row.get(2)?,
                    overrides: row.get(3)?,
                    in_cloud: row.get(4)?,
                    missing: row.get(5)?,
                })
            })
            .optional()?;
        if let Some(stored) = &stored
            && (stored.file_size, stored.date_modified) == (facts.file_size, facts.date_modified)
            // A placeholder cannot be re-read, so what is known stands until it is local.
            && (stored.parser_version >= TAG_PARSER_VERSION || dataless)
        {
            // The file is here, so a "missing" flag from an earlier check is out of date.
            if stored.in_cloud != dataless || stored.missing {
                self.conn.execute(
                    "UPDATE tracks SET in_cloud = ?2, missing = 0 WHERE path = ?1",
                    params![facts.path, dataless],
                )?;
            }
            return Ok(Outcome::Unchanged);
        }

        // Overrides outlive re-reads: a podcast's title stays what the feed said.
        let overrides: Option<TrackOverrides> = stored
            .as_ref()
            .and_then(|stored| stored.overrides.as_deref())
            .map(serde_json::from_str)
            .transpose()?;
        let track = if dataless {
            // Reading the tags would download the file, so go by its name.
            placeholder_track(canonical, root, facts, now, overrides.as_ref())
        } else {
            read_track(canonical, facts, now, overrides.as_ref())?
        };
        Ok(if stored.is_some() {
            Outcome::Updated(track)
        } else {
            Outcome::Added(track)
        })
    }

    /// Re-checks which of these tracks are placeholders. A file that has been
    /// downloaded since is read properly, and one the system has evicted is
    /// flagged while its known details stay. Returns the ids that changed.
    pub fn refresh_cloud_status(&mut self, ids: &[i64]) -> Result<Vec<i64>> {
        let now = now_unix();
        let mut updated = Vec::new();
        let mut reread = Vec::new();
        let mut evicted = Vec::new();
        {
            let mut stored = self
                .conn
                .prepare("SELECT path, in_cloud, overrides FROM tracks WHERE id = ?1")?;
            for id in ids {
                let Some((path, in_cloud, overrides)) = stored
                    .query_row([id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, bool>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    })
                    .optional()?
                else {
                    continue;
                };
                let path = Path::new(&path);
                // A file that has gone missing is a matter for the next scan.
                let Ok(facts) = FileFacts::of(path) else {
                    continue;
                };
                match (in_cloud, (self.is_dataless)(path)) {
                    (true, false) => {
                        let overrides: Option<TrackOverrides> =
                            overrides.as_deref().map(serde_json::from_str).transpose()?;
                        // Unreadable now that it is here: left flagged for the next attempt.
                        if let Ok(track) = read_track(path, facts, now, overrides.as_ref()) {
                            reread.push(track);
                            updated.push(*id);
                        }
                    }
                    (false, true) => {
                        evicted.push(*id);
                        updated.push(*id);
                    }
                    _ => {}
                }
            }
        }
        let tx = self.conn.transaction()?;
        write_tracks(&tx, WriteMode::RefreshTags, &reread)?;
        for id in evicted {
            tx.execute("UPDATE tracks SET in_cloud = 1 WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(updated)
    }

    /// Imports specific files with metadata the tags lack or get wrong, such as
    /// podcast downloads. The files are always re-read, the overrides replace any
    /// stored for the same file, and they are re-applied on every later rescan.
    /// Returns the track ids in input order; a failure on any file adds none.
    pub fn add_files(&mut self, files: &[(PathBuf, TrackOverrides)]) -> Result<Vec<i64>> {
        let now = now_unix();
        let tracks = files
            .iter()
            .map(|(path, overrides)| {
                let canonical = fs::canonicalize(path)?;
                let facts = FileFacts::of(&canonical)?;
                read_track(&canonical, facts, now, Some(overrides))
            })
            .collect::<Result<Vec<_>>>()?;

        let mut tracks = tracks;
        let media = self.media_settings()?;
        // The same file twice in one call is placed once.
        let mut placed: HashMap<String, String> = HashMap::new();
        for track in &mut tracks {
            if let Some(path) = placed.get(&track.path) {
                track.path.clone_from(path);
            } else if !self.path_in_library(Path::new(&track.path))? {
                let original = track.path.clone();
                self.place_new_file(track, &media)?;
                placed.insert(original, track.path.clone());
            }
        }

        let tx = self.conn.transaction()?;
        write_tracks(&tx, WriteMode::RefreshTags, &tracks)?;
        let mut ids = Vec::with_capacity(files.len());
        {
            // An existing row keeps its date added on refresh, so an explicit one is set here.
            let mut store = tx.prepare(
                "UPDATE tracks SET overrides = ?2, date_added = COALESCE(?3, date_added)
                 WHERE path = ?1 RETURNING id",
            )?;
            for (track, (_, overrides)) in tracks.iter().zip(files) {
                let json = (*overrides != TrackOverrides::default())
                    .then(|| serde_json::to_string(overrides))
                    .transpose()?;
                ids.push(
                    store.query_row(params![track.path, json, overrides.date_added], |row| {
                        row.get(0)
                    })?,
                );
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    fn flush(&mut self, batch: &mut Vec<Track>) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        write_tracks(&tx, WriteMode::RefreshTags, batch.iter())?;
        tx.commit()?;
        batch.clear();
        Ok(())
    }
}

pub(crate) struct FileFacts {
    pub path: String,
    pub file_size: u64,
    pub date_modified: i64,
}

impl FileFacts {
    pub(crate) fn of(canonical: &Path) -> Result<Self> {
        let path = canonical
            .to_str()
            .ok_or_else(|| Error::NonUtf8Path(canonical.to_path_buf()))?
            .to_owned();
        let metadata = fs::metadata(canonical)?;
        Ok(Self {
            path,
            file_size: metadata.len(),
            date_modified: unix_secs(metadata.modified()?),
        })
    }
}

pub(crate) fn read_track(
    canonical: &Path,
    facts: FileFacts,
    now: i64,
    overrides: Option<&TrackOverrides>,
) -> Result<Track> {
    Ok(build_track(
        tags::read(canonical)?,
        facts,
        now,
        overrides,
        false,
    ))
}

fn placeholder_track(
    canonical: &Path,
    root: Option<&Path>,
    facts: FileFacts,
    now: i64,
    overrides: Option<&TrackOverrides>,
) -> Track {
    build_track(
        placeholder_meta(canonical, root),
        facts,
        now,
        overrides,
        true,
    )
}

fn build_track(
    mut meta: TrackMeta,
    facts: FileFacts,
    now: i64,
    overrides: Option<&TrackOverrides>,
    in_cloud: bool,
) -> Track {
    let mut date_added = now;
    if let Some(overrides) = overrides {
        apply_overrides(&mut meta, overrides);
        date_added = overrides.date_added.unwrap_or(now);
    }
    Track {
        in_cloud,
        ..new_track(
            meta,
            facts.path,
            facts.file_size,
            facts.date_modified,
            date_added,
        )
    }
}

fn apply_overrides(meta: &mut TrackMeta, overrides: &TrackOverrides) {
    let text = |target: &mut String, value: &Option<String>| {
        if let Some(value) = value {
            target.clone_from(value);
        }
    };
    text(&mut meta.title, &overrides.title);
    text(&mut meta.artist, &overrides.artist);
    text(&mut meta.album, &overrides.album);
    text(&mut meta.album_artist, &overrides.album_artist);
    text(&mut meta.genre, &overrides.genre);
    if let Some(kind) = overrides.kind {
        meta.kind = kind;
    }
    if overrides.year.is_some() {
        meta.year = overrides.year;
    }
}

/// User-state columns start at their defaults; on a rescan the writer leaves
/// the stored values alone, and `id` is ignored in favour of SQLite's own.
fn new_track(
    meta: TrackMeta,
    path: String,
    file_size: u64,
    date_modified: i64,
    date_added: i64,
) -> Track {
    Track {
        id: 0,
        path,
        title: meta.title,
        artist: meta.artist,
        album_artist: meta.album_artist,
        album: meta.album,
        genre: meta.genre,
        composer: meta.composer,
        year: meta.year,
        track_number: meta.track_number,
        track_count: meta.track_count,
        disc_number: meta.disc_number,
        disc_count: meta.disc_count,
        duration_ms: meta.duration_ms,
        bitrate_kbps: meta.bitrate_kbps,
        sample_rate: meta.sample_rate,
        bit_depth: meta.bit_depth,
        channels: meta.channels,
        codec: meta.codec.to_owned(),
        file_size,
        date_added,
        date_modified,
        play_count: 0,
        last_played: None,
        skip_count: 0,
        rating: 0,
        kind: meta.kind,
        compilation: meta.compilation,
        enabled: true,
        in_cloud: false,
        missing: false,
        sort_title: meta.sort_title,
        sort_artist: meta.sort_artist,
        sort_album: meta.sort_album,
        sort_album_artist: meta.sort_album_artist,
    }
}

pub(crate) fn is_audio(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
        AUDIO_EXTENSIONS
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    })
}

/// Covers dotfiles and macOS AppleDouble (`._*`) sidecars, which carry audio
/// extensions but are not audio.
pub(crate) fn is_hidden(entry: &DirEntry) -> bool {
    entry.file_name().as_encoded_bytes().starts_with(b".")
}

pub(crate) fn now_unix() -> i64 {
    unix_secs(SystemTime::now())
}

fn unix_secs(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    }
}
