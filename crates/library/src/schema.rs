use rusqlite::Connection;

use crate::error::{Error, Result};

/// Append-only: the index plus one is the `user_version` the migration produces.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE tracks (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        title TEXT NOT NULL,
        artist TEXT NOT NULL,
        album_artist TEXT NOT NULL,
        album TEXT NOT NULL,
        genre TEXT NOT NULL,
        composer TEXT NOT NULL,
        year INTEGER,
        track_number INTEGER,
        track_count INTEGER,
        disc_number INTEGER,
        disc_count INTEGER,
        duration_ms INTEGER NOT NULL,
        bitrate_kbps INTEGER,
        sample_rate INTEGER,
        bit_depth INTEGER,
        channels INTEGER,
        codec TEXT NOT NULL,
        file_size INTEGER NOT NULL,
        date_added INTEGER NOT NULL,
        date_modified INTEGER NOT NULL,
        play_count INTEGER NOT NULL DEFAULT 0,
        last_played INTEGER,
        skip_count INTEGER NOT NULL DEFAULT 0,
        last_skipped INTEGER,
        rating INTEGER NOT NULL DEFAULT 0,
        kind INTEGER NOT NULL,
        compilation INTEGER NOT NULL,
        sort_title TEXT,
        sort_artist TEXT,
        sort_album TEXT,
        sort_album_artist TEXT,
        -- Folded sort keys are NULL when empty so `NULLS LAST` sorts blanks at the end.
        key_title TEXT,
        key_artist TEXT,
        key_album TEXT,
        key_album_artist TEXT,
        key_genre TEXT,
        search_text TEXT NOT NULL
    );",
    // Rows from before the column existed were read by parser version 1.
    "ALTER TABLE tracks ADD COLUMN parser_version INTEGER NOT NULL DEFAULT 1;",
];

pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let supported = MIGRATIONS.len() as u32;
    if current > supported {
        return Err(Error::SchemaTooNew {
            found: current,
            supported,
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}
