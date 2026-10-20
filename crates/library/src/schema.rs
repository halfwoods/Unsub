use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::playlist::seed_defaults;

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
    // The two group columns are backfilled with the SQL twin of
    // `normalize::artist_group` / `album_group`; `fold` is registered before
    // migrations run.
    "ALTER TABLE tracks ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
    ALTER TABLE tracks ADD COLUMN artist_group TEXT NOT NULL DEFAULT '';
    ALTER TABLE tracks ADD COLUMN album_group TEXT NOT NULL DEFAULT '';
    UPDATE tracks SET artist_group = CASE WHEN compilation THEN 'compilations'
        ELSE fold(CASE WHEN album_artist = '' THEN artist ELSE album_artist END) END;
    UPDATE tracks SET album_group = artist_group || char(31) || fold(album);
    CREATE INDEX tracks_artist_group ON tracks(artist_group);
    CREATE INDEX tracks_album_group ON tracks(album_group);
    CREATE TABLE playlists (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        kind INTEGER NOT NULL,
        parent_id INTEGER REFERENCES playlists(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        rules TEXT,
        seed INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE playlist_items (
        playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
        PRIMARY KEY (playlist_id, position)
    ) WITHOUT ROWID;
    CREATE INDEX playlist_items_track ON playlist_items(track_id);",
    // `kind` is 0 for a play and 1 for a skip. `overrides` holds the JSON of a
    // `TrackOverrides`, null for the vast majority of tracks.
    "CREATE TABLE play_events (
        track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
        at INTEGER NOT NULL,
        kind INTEGER NOT NULL
    );
    CREATE INDEX play_events_at ON play_events(at);
    CREATE INDEX play_events_track ON play_events(track_id);
    ALTER TABLE tracks ADD COLUMN overrides TEXT;",
    "ALTER TABLE tracks ADD COLUMN in_cloud INTEGER NOT NULL DEFAULT 0;",
    "ALTER TABLE tracks ADD COLUMN missing INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;",
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
    let fresh = current == 0;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        // Only a database created by this build gets the stock playlists; one
        // being upgraded may have been through years of the user's own.
        if fresh && index + 1 == MIGRATIONS.len() {
            seed_defaults(&tx)?;
        }
        tx.pragma_update(None, "user_version", index as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}
