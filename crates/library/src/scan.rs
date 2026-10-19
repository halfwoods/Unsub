use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;
use walkdir::{DirEntry, WalkDir};

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{ScanProgress, ScanSummary, Track};
use crate::store::{WriteMode, write_tracks};
use crate::tags::{self, TAG_PARSER_VERSION, TrackMeta};

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "m4b", "mp4", "aac", "flac", "ogg", "oga", "opus", "wav", "aif", "aiff", "wv",
    "ape",
];
const BATCH_SIZE: usize = 500;
const PROGRESS_INTERVAL: u64 = 50;

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

        for root in paths {
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
                match self.examine(&canonical, now) {
                    Ok(Outcome::Unchanged) => summary.unchanged += 1,
                    Ok(Outcome::Added(track)) => {
                        summary.added += 1;
                        batch.push(track);
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

    fn examine(&self, canonical: &Path, now: i64) -> Result<Outcome> {
        let path = canonical
            .to_str()
            .ok_or_else(|| Error::NonUtf8Path(canonical.to_path_buf()))?
            .to_owned();
        let metadata = fs::metadata(canonical)?;
        let file_size = metadata.len();
        let date_modified = unix_secs(metadata.modified()?);

        let stored: Option<(u64, i64, u32)> = self
            .conn
            .prepare_cached(
                "SELECT file_size, date_modified, parser_version FROM tracks WHERE path = ?1",
            )?
            .query_row([&path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
        if let Some((size, modified, parser_version)) = stored
            && (size, modified) == (file_size, date_modified)
            && parser_version >= TAG_PARSER_VERSION
        {
            return Ok(Outcome::Unchanged);
        }

        let track = new_track(tags::read(canonical)?, path, file_size, date_modified, now);
        Ok(if stored.is_some() {
            Outcome::Updated(track)
        } else {
            Outcome::Added(track)
        })
    }

    fn flush(&mut self, batch: &mut Vec<Track>) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        write_tracks(&mut self.conn, WriteMode::RefreshTags, batch.iter())?;
        batch.clear();
        Ok(())
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
        sort_title: meta.sort_title,
        sort_artist: meta.sort_artist,
        sort_album: meta.sort_album,
        sort_album_artist: meta.sort_album_artist,
    }
}

fn is_audio(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
        AUDIO_EXTENSIONS
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    })
}

/// Covers dotfiles and macOS AppleDouble (`._*`) sidecars, which carry audio
/// extensions but are not audio.
fn is_hidden(entry: &DirEntry) -> bool {
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
