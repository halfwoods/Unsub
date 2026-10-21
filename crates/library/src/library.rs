use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::functions::FunctionFlags;
use rusqlite::vtab::array;
use rusqlite::{Connection, OptionalExtension, Statement, params};

use crate::cloud::platform_is_dataless;
use crate::delete::move_to_trash;
use crate::error::{Error, Result};
use crate::model::{ExternalStats, Track};
use crate::normalize::{artist_key, fold, song_key};
use crate::schema;
use crate::store::{WriteMode, write_tracks};

/// Two services recording one listen can differ by its length (one stamps the start, one the end),
/// and a little more for clocks.
const ELSEWHERE_SLACK_SECONDS: i64 = 120;

/// `play_events.kind` values.
pub(crate) const PLAY_EVENT: i64 = 0;
pub(crate) const SKIP_EVENT: i64 = 1;

type TrashFn = dyn Fn(&Path) -> std::io::Result<()> + Send;

/// An older build refuses a database that has been given schema 13, so the library as it was is kept once,
/// beside it, for going back. A fresh database has nothing to keep, and a copy that can't be made (the disk
/// is full) must not keep the library from opening.
fn keep_before_thirteen(conn: &Connection, db_path: &Path) {
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0);
    let Some(name) = db_path.file_name() else {
        return;
    };
    let name = name.to_string_lossy();
    let backup = db_path.with_file_name(format!("{name}.before-13"));
    if !(1..13).contains(&version) || backup.exists() {
        return;
    }
    // Made under another name and renamed when whole, so that a crash halfway leaves nothing that
    // passes for a backup, and a half-made one from an earlier crash is cleared away first.
    let partial = db_path.with_file_name(format!("{name}.before-13.tmp"));
    let _ = fs::remove_file(&partial);
    let kept = conn
        .execute("VACUUM INTO ?1", [partial.to_string_lossy()])
        .is_ok()
        && fs::rename(&partial, &backup).is_ok();
    if !kept {
        let _ = fs::remove_file(partial);
    }
}

pub struct Library {
    pub(crate) conn: Connection,
    /// Where `artwork_thumbnail` keeps resized images; unset means no caching.
    pub(crate) artwork_cache: Option<PathBuf>,
    /// Where the covers of songs that are only in iCloud are kept for good. They are the only copy
    /// (the files are not here to read them from), so unlike the cache they are not to be lost.
    pub(crate) cloud_covers: Option<PathBuf>,
    /// Tells whether a file is only a placeholder for contents in the cloud.
    pub(crate) is_dataless: Box<dyn Fn(&Path) -> bool + Send>,
    /// Moves a file to the Trash.
    pub(crate) trash: Box<TrashFn>,
}

impl Library {
    pub fn open(db_path: &Path) -> Result<Library> {
        if let Some(parent) = db_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        // WAL keeps readers (UI queries) unblocked while a scan commits batches.
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        keep_before_thirteen(&conn, db_path);
        let mut library = Self::init(conn)?;
        // Beside the database, so they move with it.
        library.cloud_covers = Some(db_path.with_file_name("Covers"));
        Ok(library)
    }

    /// How long a statement waits for another connection's write to finish before it fails; work
    /// that goes on for hours would rather wait longer than end.
    pub fn set_busy_timeout(&self, timeout: Duration) -> Result<()> {
        Ok(self.conn.busy_timeout(timeout)?)
    }

    pub fn open_in_memory() -> Result<Library> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Library> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        // Backs the `ids` filter, which can exceed SQLite's bound-variable limit.
        array::load_module(&conn)?;
        // Smart-playlist text rules and the grouping backfill compare folded text.
        conn.create_scalar_function(
            "fold",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| Ok(ctx.get_raw(0).as_str_or_null()?.map(fold)),
        )?;
        schema::migrate(&mut conn)?;
        Ok(Library {
            conn,
            artwork_cache: None,
            cloud_covers: None,
            is_dataless: Box::new(platform_is_dataless),
            trash: Box::new(move_to_trash),
        })
    }

    /// Replaces how a file is recognised as a cloud placeholder. By default the
    /// operating system is asked (only macOS has any placeholders); a test, or a
    /// platform with its own kind, can supply a different test.
    pub fn set_dataless_check(&mut self, check: impl Fn(&Path) -> bool + Send + 'static) {
        self.is_dataless = Box::new(check);
    }

    /// Loads rows from another source (a device database) keeping their own
    /// ids, which other data refers to. Nothing on disk is read: `path` is
    /// stored as given.
    pub fn import_tracks(&mut self, tracks: &[Track]) -> Result<()> {
        let tx = self.conn.transaction()?;
        write_tracks(&tx, WriteMode::Replace, tracks)?;
        tx.commit()?;
        Ok(())
    }

    /// The songs among `source_keys` that `copy_files_in` or `restore_from` has put in the library,
    /// with their track ids. An entry goes when its song does.
    pub fn rescued(&self, source_keys: &[String]) -> Result<HashMap<String, i64>> {
        let mut find = self
            .conn
            .prepare_cached("SELECT track_id FROM rescued WHERE source_key = ?1")?;
        let mut found = HashMap::new();
        for key in source_keys {
            if let Some(id) = find.query_row([key], |row| row.get(0)).optional()? {
                found.insert(key.clone(), id);
            }
        }
        Ok(found)
    }

    pub fn record_play(&mut self, track_id: i64, played_at_unix: i64) -> Result<()> {
        self.record_event(
            track_id,
            played_at_unix,
            PLAY_EVENT,
            "UPDATE tracks SET play_count = play_count + 1, last_played = ?2 WHERE id = ?1",
        )
    }

    pub fn record_skip(&mut self, track_id: i64, skipped_at_unix: i64) -> Result<()> {
        self.record_event(
            track_id,
            skipped_at_unix,
            SKIP_EVENT,
            "UPDATE tracks SET skip_count = skip_count + 1, last_skipped = ?2 WHERE id = ?1",
        )
    }

    /// Adds plays that happened elsewhere, as `(track id, unix time)`, tagged with the `source`
    /// that gave them, and says how many were new. A play is not new when this source already gave
    /// that song at that time (importing a history twice, or after the song was matched to
    /// another copy). Nor is it when it is the same listen as one a different source recorded,
    /// since two services recording one listen stamp it at its start or its end: within the
    /// song's length plus two minutes of such an event, which it then claims. An event is claimed
    /// by at most one play of a source, the nearest, so three real plays are not made one by a
    /// single listen elsewhere. All or nothing.
    pub fn record_plays_from(&mut self, source: &str, plays: &[(i64, i64)]) -> Result<usize> {
        // Nearest-first pairing is the same whatever order the caller had them in.
        let mut plays = plays.to_vec();
        plays.sort_unstable_by_key(|&(track_id, at)| (at, track_id));
        let tx = self.conn.transaction()?;
        let mut added = 0;
        {
            let mut song = tx.prepare(
                "SELECT title, artist, album_artist, duration_ms FROM tracks WHERE id = ?1",
            )?;
            let mut logged = tx.prepare(
                "SELECT 1 FROM play_events WHERE source = ?1 AND at = ?2 AND song_key = ?3",
            )?;
            let mut claimed = tx.prepare(
                "SELECT 1 FROM play_duplicates WHERE source = ?1 AND at = ?2 AND song_key = ?3",
            )?;
            let mut nearest = tx.prepare(
                "SELECT rowid FROM play_events
                 WHERE song_key = ?1 AND source IS NOT NULL AND source <> ?2
                    AND at BETWEEN ?3 AND ?4
                    AND rowid NOT IN (SELECT of_rowid FROM play_duplicates WHERE source = ?2)
                 ORDER BY ABS(at - ?5), rowid LIMIT 1",
            )?;
            let mut claim = tx.prepare(
                "INSERT INTO play_duplicates (source, at, song_key, of_rowid)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut log = tx.prepare(
                "INSERT INTO play_events (track_id, at, kind, source, song_key)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            let mut count = tx.prepare(
                "UPDATE tracks SET play_count = play_count + 1,
                    last_played = CASE WHEN last_played IS NULL THEN ?2
                        ELSE MAX(last_played, ?2) END
                 WHERE id = ?1",
            )?;
            let mut known: HashMap<i64, (String, i64)> = HashMap::new();
            for (track_id, at) in plays {
                let (key, window) = &*match known.entry(track_id) {
                    Entry::Occupied(known) => known.into_mut(),
                    Entry::Vacant(unknown) => unknown.insert(identity(&mut song, track_id)?),
                };
                if logged.exists(params![source, at, key])?
                    || claimed.exists(params![source, at, key])?
                {
                    continue;
                }
                let same_listen: Option<i64> = nearest
                    .query_row(
                        params![key, source, at - *window, at + *window, at],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(event) = same_listen {
                    claim.execute(params![source, at, key, event])?;
                    continue;
                }
                log.execute(params![track_id, at, PLAY_EVENT, source, key])?;
                require_changed(count.execute(params![track_id, at])?, track_id)?;
                added += 1;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// Plays at or after `since`, newest first, as `(track id, unix time)`.
    /// Skips are logged too but are not plays, so they are left out.
    pub fn recent_plays(&self, since: i64) -> Result<Vec<(i64, i64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT track_id, at FROM play_events WHERE kind = ?1 AND at >= ?2
             ORDER BY at DESC, rowid DESC",
        )?;
        let plays = stmt.query_map(params![PLAY_EVENT, since], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        Ok(plays.collect::<rusqlite::Result<_>>()?)
    }

    /// The counters answer "how many"; the log keeps "when", for history and
    /// for learning which songs are played together.
    fn record_event(&mut self, track_id: i64, at: i64, kind: i64, update: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(update, params![track_id, at])?;
        require_changed(changed, track_id)?;
        tx.execute(
            "INSERT INTO play_events (track_id, at, kind) VALUES (?1, ?2, ?3)",
            params![track_id, at, kind],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_rating(&mut self, track_id: i64, rating: u8) -> Result<()> {
        if rating > 100 || !rating.is_multiple_of(20) {
            return Err(Error::InvalidRating(rating));
        }
        let changed = self.conn.execute(
            "UPDATE tracks SET rating = ?2 WHERE id = ?1",
            params![track_id, rating],
        )?;
        require_changed(changed, track_id)
    }

    /// The song checkbox.
    pub fn set_enabled(&mut self, ids: &[i64], enabled: bool) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut update = tx.prepare("UPDATE tracks SET enabled = ?2 WHERE id = ?1")?;
            for id in ids {
                update.execute(params![id, enabled])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Folds play data from another library into ours without losing either
    /// side's history: counts and dates keep the larger or later value, the
    /// earliest date added wins, a rating fills in only where we have none,
    /// and the checkbox follows the other library. Plays logged here, by Unsub
    /// or by a streaming service (`record_plays_from`), are plays the other
    /// library never saw, so they are added to its count before comparing:
    /// importing again stays the same, whichever import came first. All or nothing.
    pub fn apply_external_stats(&mut self, stats: &[(i64, ExternalStats)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut merge = tx.prepare(
                "UPDATE tracks SET
                    play_count = MAX(play_count, ?2 + (SELECT COUNT(*) FROM play_events
                        WHERE track_id = ?1 AND kind = 0)),
                    skip_count = MAX(skip_count, ?3),
                    last_played = CASE WHEN ?4 IS NULL THEN last_played
                        WHEN last_played IS NULL THEN ?4 ELSE MAX(last_played, ?4) END,
                    last_skipped = CASE WHEN ?5 IS NULL THEN last_skipped
                        WHEN last_skipped IS NULL THEN ?5 ELSE MAX(last_skipped, ?5) END,
                    rating = CASE WHEN rating = 0 THEN ?6 ELSE rating END,
                    date_added = CASE WHEN ?7 IS NULL THEN date_added ELSE MIN(date_added, ?7) END,
                    enabled = ?8
                 WHERE id = ?1",
            )?;
            for (id, theirs) in stats {
                let changed = merge.execute(params![
                    id,
                    theirs.play_count,
                    theirs.skip_count,
                    theirs.last_played,
                    theirs.last_skipped,
                    theirs.rating.min(100),
                    theirs.date_added,
                    theirs.enabled,
                ])?;
                require_changed(changed, *id)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Notes where playback of a track has got to, so it can be resumed. Only
    /// tracks that remember their position (audiobooks and podcasts unless Get
    /// Info says otherwise) keep one. Reaching the last ten seconds, or the
    /// start, means there is nothing to resume, so the bookmark is cleared.
    pub fn set_bookmark(&mut self, id: i64, position_ms: u64) -> Result<()> {
        let (remembers, duration_ms) = self
            .position_settings(id)?
            .ok_or(Error::TrackNotFound(id))?;
        if !remembers {
            return Ok(());
        }
        let bookmark = Some(position_ms).filter(|ms| !is_finished(*ms, duration_ms));
        self.conn.execute(
            "UPDATE tracks SET bookmark_ms = ?2 WHERE id = ?1",
            params![id, bookmark],
        )?;
        Ok(())
    }

    /// Where to resume, if there is somewhere. A bookmark is ignored once the
    /// track stops remembering, or if the track has since changed so that the
    /// position is at its end.
    pub fn bookmark(&self, id: i64) -> Result<Option<u64>> {
        let Some((remembers, duration_ms)) = self.position_settings(id)? else {
            return Ok(None);
        };
        let stored: Option<u64> = self.conn.query_row(
            "SELECT bookmark_ms FROM tracks WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        Ok(stored.filter(|ms| remembers && !is_finished(*ms, duration_ms)))
    }

    pub fn clear_bookmark(&mut self, id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE tracks SET bookmark_ms = NULL WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Whether the track remembers its position, and how long it is.
    fn position_settings(&self, id: i64) -> Result<Option<(bool, u64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT COALESCE(remember_position, kind IN (1, 2)), duration_ms
                 FROM tracks WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    pub fn remove_tracks(&mut self, ids: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut delete = tx.prepare("DELETE FROM tracks WHERE id = ?1")?;
            for id in ids {
                delete.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

/// "Remember playback position" lets go of a place that is the start, or within
/// this long of the end, where picking up again would be pointless.
const FINISHED_WITHIN_MS: u64 = 10_000;

fn is_finished(position_ms: u64, duration_ms: u64) -> bool {
    position_ms == 0
        || (duration_ms > 0 && position_ms.saturating_add(FINISHED_WITHIN_MS) >= duration_ms)
}

/// What `record_plays_from` knows a track by: its song key, and how many seconds either side of
/// a listen another service may have stamped it.
fn identity(song: &mut Statement<'_>, track_id: i64) -> Result<(String, i64)> {
    let (title, artist, album_artist, duration_ms): (String, String, String, i64) = song
        .query_row([track_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?
        .ok_or(Error::TrackNotFound(track_id))?;
    let artist = if artist_key(&artist).is_some() {
        artist
    } else {
        album_artist
    };
    Ok((
        song_key(&title, &artist),
        duration_ms / 1_000 + ELSEWHERE_SLACK_SECONDS,
    ))
}

fn require_changed(changed: usize, track_id: i64) -> Result<()> {
    if changed == 0 {
        return Err(Error::TrackNotFound(track_id));
    }
    Ok(())
}
