use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::functions::FunctionFlags;
use rusqlite::vtab::array;
use rusqlite::{Connection, params};

use crate::cloud::platform_is_dataless;
use crate::delete::move_to_trash;
use crate::error::{Error, Result};
use crate::model::{ExternalStats, Track};
use crate::normalize::fold;
use crate::schema;
use crate::store::{WriteMode, write_tracks};

/// `play_events.kind` values.
pub(crate) const PLAY_EVENT: i64 = 0;
pub(crate) const SKIP_EVENT: i64 = 1;

type TrashFn = dyn Fn(&Path) -> std::io::Result<()> + Send;

pub struct Library {
    pub(crate) conn: Connection,
    /// Where `artwork_thumbnail` keeps resized images; unset means no caching.
    pub(crate) artwork_cache: Option<PathBuf>,
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
        Self::init(conn)
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
    /// and the checkbox follows the other library. All or nothing.
    pub fn apply_external_stats(&mut self, stats: &[(i64, ExternalStats)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut merge = tx.prepare(
                "UPDATE tracks SET
                    play_count = MAX(play_count, ?2),
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

fn require_changed(changed: usize, track_id: i64) -> Result<()> {
    if changed == 0 {
        return Err(Error::TrackNotFound(track_id));
    }
    Ok(())
}
