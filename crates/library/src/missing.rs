use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{RADIO, Track};
use crate::normalize::{artist_key, title_key};
use crate::organize::{copy_file, organized_path, prune_empty_dirs};
use crate::scan::{FileFacts, is_audio, is_hidden};
use crate::tags;

/// Rips and re-encodes differ by a moment either way.
const SAME_SONG_MS: u64 = 2_000;

struct MissingTrack {
    id: i64,
    path: String,
    file_size: u64,
    title: String,
    artist: String,
    duration_ms: u64,
}

impl Library {
    /// Looks for every track's file and flags those that are gone, clearing
    /// the flag on any that have come back. Placeholders still in the cloud are
    /// not checked. Returns all the tracks now flagged.
    pub fn check_missing(&mut self) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, path, missing FROM tracks WHERE in_cloud = 0 AND kind <> {RADIO}
                 ORDER BY id"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?;
        let mut missing = Vec::new();
        let mut changes = Vec::new();
        for row in rows {
            let (id, path, was_missing) = row?;
            // A file that cannot be inspected for another reason keeps its flag.
            let gone = match fs::metadata(&path) {
                Ok(_) => Some(false),
                Err(err) if is_absent(&err) => Some(true),
                Err(_) => None,
            };
            if gone == Some(true) {
                missing.push(id);
            }
            if let Some(gone) = gone.filter(|gone| *gone != was_missing) {
                changes.push((id, gone));
            }
        }
        drop(stmt);

        let tx = self.conn.transaction()?;
        {
            let mut update = tx.prepare("UPDATE tracks SET missing = ?2 WHERE id = ?1")?;
            for (id, gone) in changes {
                update.execute(params![id, gone])?;
            }
        }
        tx.commit()?;
        Ok(missing)
    }

    /// Points a track at a file that stands in for its lost one. The library's
    /// details are kept, not re-read from the new file.
    pub fn relink(&mut self, id: i64, new_path: &Path) -> Result<()> {
        self.track(id)?.ok_or(Error::TrackNotFound(id))?;
        let canonical = fs::canonicalize(new_path)?;
        if !canonical.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file").into());
        }
        self.relink_to(id, &canonical)
    }

    /// Puts a copy of `source` in the media folder, at the path the track's details file it under,
    /// and points the track at it, for a track whose own file is lost and whose song is at hand on
    /// a disk that will be unplugged. The library's tags, overrides, counts and playlists are
    /// kept, but the audio details (codec, bit rate, sample rate, bit depth, channels, length) are
    /// those of the copy, which need not be the lost file's format, and the stored loudness goes
    /// so Sound Check measures the copy. `source` is only read, the copy is whole or not at all,
    /// and `source_key` is recorded for `rescued`.
    ///
    /// Returns `false`, having changed nothing, if the track's own file is there after all: a
    /// disk that was only unplugged has come back, and is not lost.
    pub fn restore_from(&mut self, id: i64, source: &Path, source_key: &str) -> Result<bool> {
        let track = self.track(id)?.ok_or(Error::TrackNotFound(id))?;
        if fs::metadata(&track.path).is_ok() {
            return Ok(false);
        }
        let folder = self.media_settings()?.folder.ok_or(Error::NoMediaFolder)?;
        // The name takes its extension from the file it holds, which is not the lost one's.
        let filed = Track {
            path: source.to_string_lossy().into_owned(),
            ..track
        };
        let target = self.free_path(organized_path(&folder, &filed), None)?;
        let pointed = copy_file(source, &target)
            .map_err(Error::from)
            .and_then(|()| tags::read(&target))
            .and_then(|meta| {
                self.relink(id, &target)?;
                Ok(meta)
            });
        let meta = match pointed {
            Ok(meta) => meta,
            Err(err) => {
                let _ = fs::remove_file(&target);
                if let Some(parent) = target.parent() {
                    prune_empty_dirs(&folder, parent);
                }
                return Err(err);
            }
        };
        let loudness = meta.loudness.as_ref();
        self.conn.execute(
            "UPDATE tracks SET duration_ms = ?2, bitrate_kbps = ?3, sample_rate = ?4, bit_depth = ?5,
                               channels = ?6, codec = ?7, loudness_lufs = ?8, true_peak_dbtp = ?9,
                               loudness_source = ?10, album_loudness_lufs = ?11,
                               album_true_peak_dbtp = ?12, loudness_state = NULL
             WHERE id = ?1",
            params![
                id,
                meta.duration_ms,
                meta.bitrate_kbps,
                meta.sample_rate,
                meta.bit_depth,
                meta.channels,
                meta.codec,
                loudness.map(|l| l.lufs),
                loudness.and_then(|l| l.true_peak_dbtp),
                loudness.map(|l| l.source),
                loudness.and_then(|l| l.album_lufs),
                loudness.and_then(|l| l.album_true_peak_dbtp),
            ],
        )?;
        self.conn.execute(
            "INSERT OR REPLACE INTO rescued (source_key, track_id) VALUES (?1, ?2)",
            params![source_key, id],
        )?;
        Ok(true)
    }

    /// iTunes' "locate other missing files": every missing track whose file name
    /// turns up under `folder`, and whose size or tags confirm it is the same
    /// song. Returns the tracks relinked.
    pub fn relink_folder(&mut self, folder: &Path) -> Result<Vec<i64>> {
        let lost = self.missing_tracks()?;
        if lost.is_empty() {
            return Ok(Vec::new());
        }
        let mut by_name: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let walker = WalkDir::new(folder)
            .follow_links(true)
            .into_iter()
            .filter_entry(|entry| entry.depth() == 0 || !is_hidden(entry));
        for entry in walker.filter_map(std::result::Result::ok) {
            if entry.file_type().is_file()
                && is_audio(entry.path())
                && let Ok(canonical) = fs::canonicalize(entry.path())
            {
                let name = file_name(&canonical);
                by_name.entry(name).or_default().push(canonical);
            }
        }
        for candidates in by_name.values_mut() {
            candidates.sort();
        }

        let mut taken: HashSet<PathBuf> = HashSet::new();
        let mut relinked = Vec::new();
        for track in lost {
            let Some(candidates) = by_name.get(&file_name(Path::new(&track.path))) else {
                continue;
            };
            let mut free = Vec::new();
            for candidate in candidates {
                if !taken.contains(candidate) && !self.path_in_library(candidate)? {
                    free.push(candidate);
                }
            }
            let same_size = free.iter().find(|candidate| {
                fs::metadata(candidate).is_ok_and(|m| m.len() == track.file_size)
            });
            // Reading a file's tags would download it, so one only in the cloud is known by size alone.
            let found = same_size
                .or_else(|| {
                    free.iter().find(|candidate| {
                        !(self.is_dataless)(candidate) && same_song(&track, candidate)
                    })
                })
                .map(|candidate| (*candidate).clone());
            if let Some(found) = found {
                self.relink_to(track.id, &found)?;
                taken.insert(found);
                relinked.push(track.id);
            }
        }
        Ok(relinked)
    }

    fn missing_tracks(&self) -> Result<Vec<MissingTrack>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, file_size, title, artist, duration_ms
             FROM tracks WHERE missing = 1 ORDER BY id",
        )?;
        let tracks = stmt.query_map([], |row| {
            Ok(MissingTrack {
                id: row.get(0)?,
                path: row.get(1)?,
                file_size: row.get(2)?,
                title: row.get(3)?,
                artist: row.get(4)?,
                duration_ms: row.get(5)?,
            })
        })?;
        Ok(tracks.collect::<rusqlite::Result<_>>()?)
    }

    fn relink_to(&self, id: i64, canonical: &Path) -> Result<()> {
        let facts = FileFacts::of(canonical)?;
        let holder: Option<i64> = self
            .conn
            .prepare_cached("SELECT id FROM tracks WHERE path = ?1")?
            .query_row([&facts.path], |row| row.get(0))
            .optional()?;
        if holder.is_some_and(|holder| holder != id) {
            return Err(Error::PathInUse(facts.path));
        }
        self.conn.execute(
            "UPDATE tracks SET path = ?2, file_size = ?3, date_modified = ?4, missing = 0,
                               in_cloud = ?5
             WHERE id = ?1",
            params![
                id,
                facts.path,
                facts.file_size,
                facts.date_modified,
                (self.is_dataless)(canonical)
            ],
        )?;
        Ok(())
    }
}

fn is_absent(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The file's own tags agree with the library's about which song it is.
fn same_song(track: &MissingTrack, candidate: &Path) -> bool {
    let Ok(meta) = tags::read(candidate) else {
        return false;
    };
    let (Some(title), Some(artist)) = (title_key(&track.title), artist_key(&track.artist)) else {
        return false;
    };
    title_key(&meta.title) == Some(title)
        && artist_key(&meta.artist) == Some(artist)
        && (track.duration_ms == 0 || meta.duration_ms.abs_diff(track.duration_ms) <= SAME_SONG_MS)
}
