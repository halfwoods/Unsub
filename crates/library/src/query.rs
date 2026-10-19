use std::rc::Rc;

use rusqlite::types::{ToSql, Value};
use rusqlite::{OptionalExtension, Row, params_from_iter};

use crate::error::Result;
use crate::library::Library;
use crate::model::{LibraryStats, SortKey, Track, TrackQuery};
use crate::normalize::fold;

const TRACK_COLUMNS: &str = "id, path, title, artist, album_artist, album, genre, composer, year,
    track_number, track_count, disc_number, disc_count, duration_ms, bitrate_kbps, sample_rate,
    bit_depth, channels, codec, file_size, date_added, date_modified, play_count, last_played,
    skip_count, rating, kind, compilation, sort_title, sort_artist, sort_album, sort_album_artist";

impl Library {
    pub fn tracks(&self, query: &TrackQuery) -> Result<Vec<Track>> {
        let (filter, params) = filter(query);
        let sql = format!(
            "SELECT {TRACK_COLUMNS} FROM tracks {filter} ORDER BY {}",
            order_by(query.sort, query.descending)
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), track_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn track(&self, id: i64) -> Result<Option<Track>> {
        let sql = format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?1");
        Ok(self
            .conn
            .prepare_cached(&sql)?
            .query_row([id], track_from_row)
            .optional()?)
    }

    pub fn stats(&self, query: &TrackQuery) -> Result<LibraryStats> {
        let (filter, params) = filter(query);
        let sql = format!(
            "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0), COALESCE(SUM(file_size), 0)
             FROM tracks {filter}"
        );
        Ok(self
            .conn
            .prepare_cached(&sql)?
            .query_row(params_from_iter(params), |row| {
                Ok(LibraryStats {
                    count: row.get(0)?,
                    total_duration_ms: row.get(1)?,
                    total_bytes: row.get(2)?,
                })
            })?)
    }
}

/// Positional access: by-name lookup rescans the column list per field, which
/// dominates the cost of materialising a 100k-row library.
fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        album_artist: row.get(4)?,
        album: row.get(5)?,
        genre: row.get(6)?,
        composer: row.get(7)?,
        year: row.get(8)?,
        track_number: row.get(9)?,
        track_count: row.get(10)?,
        disc_number: row.get(11)?,
        disc_count: row.get(12)?,
        duration_ms: row.get(13)?,
        bitrate_kbps: row.get(14)?,
        sample_rate: row.get(15)?,
        bit_depth: row.get(16)?,
        channels: row.get(17)?,
        codec: row.get(18)?,
        file_size: row.get(19)?,
        date_added: row.get(20)?,
        date_modified: row.get(21)?,
        play_count: row.get(22)?,
        last_played: row.get(23)?,
        skip_count: row.get(24)?,
        rating: row.get(25)?,
        kind: row.get(26)?,
        compilation: row.get(27)?,
        sort_title: row.get(28)?,
        sort_artist: row.get(29)?,
        sort_album: row.get(30)?,
        sort_album_artist: row.get(31)?,
    })
}

/// Everything user-supplied is bound as a parameter. `instr` avoids LIKE's
/// wildcard escaping, and `rarray` carries the id list as a single parameter.
fn filter(query: &TrackQuery) -> (String, Vec<Box<dyn ToSql>>) {
    let mut clauses = Vec::new();
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(kind) = query.kind {
        clauses.push("kind = ?");
        params.push(Box::new(kind));
    }
    if let Some(search) = &query.search {
        for word in fold(search).split_whitespace() {
            clauses.push("instr(search_text, ?) > 0");
            params.push(Box::new(word.to_owned()));
        }
    }
    if let Some(ids) = &query.ids {
        clauses.push("id IN (SELECT value FROM rarray(?))");
        let values: Vec<Value> = ids.iter().copied().map(Value::Integer).collect();
        params.push(Box::new(Rc::new(values)));
    }
    let clause = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    (clause, params)
}

/// Only the primary key honours `descending`, matching iTunes; the tie-break
/// columns always ascend. Column names come from this closed set, never from
/// user input, so interpolating them into SQL is safe.
fn order_by(sort: SortKey, descending: bool) -> String {
    const BY_ARTIST: &[&str] = &["key_artist", "key_album", "disc_number", "track_number"];
    let (primary, tail): (&str, &[&str]) = match sort {
        SortKey::Artist => (
            "key_artist",
            &["key_album", "disc_number", "track_number", "key_title"],
        ),
        SortKey::Album => ("key_album", &["disc_number", "track_number", "key_title"]),
        SortKey::AlbumArtist => (
            "key_album_artist",
            &["key_album", "disc_number", "track_number"],
        ),
        SortKey::Genre => ("key_genre", BY_ARTIST),
        SortKey::Name => ("key_title", BY_ARTIST),
        SortKey::Time => ("duration_ms", BY_ARTIST),
        SortKey::Year => ("year", BY_ARTIST),
        SortKey::Rating => ("rating", BY_ARTIST),
        SortKey::Plays => ("play_count", BY_ARTIST),
        SortKey::LastPlayed => ("last_played", BY_ARTIST),
        SortKey::DateAdded => ("date_added", BY_ARTIST),
        SortKey::Kind => ("codec COLLATE NOCASE", BY_ARTIST),
        SortKey::Bitrate => ("bitrate_kbps", BY_ARTIST),
        SortKey::SampleRate => ("sample_rate", BY_ARTIST),
        SortKey::Size => ("file_size", BY_ARTIST),
    };
    let direction = if descending { "DESC" } else { "ASC" };
    let mut clauses = vec![format!("{primary} {direction} NULLS LAST")];
    clauses.extend(tail.iter().map(|column| format!("{column} ASC NULLS LAST")));
    clauses.push("id ASC".to_owned());
    clauses.join(", ")
}
