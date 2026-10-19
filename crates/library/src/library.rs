use std::fs;
use std::path::Path;

use rusqlite::vtab::array;
use rusqlite::{Connection, params};

use crate::error::{Error, Result};
use crate::model::Track;
use crate::schema;
use crate::store::{WriteMode, write_tracks};

pub struct Library {
    pub(crate) conn: Connection,
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
        schema::migrate(&mut conn)?;
        Ok(Library { conn })
    }

    /// Loads rows from another source (a device database) keeping their own
    /// ids, which other data refers to. Nothing on disk is read: `path` is
    /// stored as given.
    pub fn import_tracks(&mut self, tracks: &[Track]) -> Result<()> {
        write_tracks(&mut self.conn, WriteMode::Replace, tracks)
    }

    pub fn record_play(&mut self, track_id: i64, played_at_unix: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE tracks SET play_count = play_count + 1, last_played = ?2 WHERE id = ?1",
            params![track_id, played_at_unix],
        )?;
        require_changed(changed, track_id)
    }

    pub fn record_skip(&mut self, track_id: i64, skipped_at_unix: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE tracks SET skip_count = skip_count + 1, last_skipped = ?2 WHERE id = ?1",
            params![track_id, skipped_at_unix],
        )?;
        require_changed(changed, track_id)
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
