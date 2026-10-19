use std::rc::Rc;

use rusqlite::types::{ToSql, Value};
use rusqlite::{Connection, OptionalExtension, Row, params_from_iter};

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{LibraryStats, SortKey, Track, TrackQuery};
use crate::normalize::fold;
use crate::playlist::{members_sql, playlist_row};
use crate::scan::now_unix;
use crate::smart::Compile;

pub(crate) const TRACK_COLUMNS: &str =
    "id, path, title, artist, album_artist, album, genre, composer, year,
    track_number, track_count, disc_number, disc_count, duration_ms, bitrate_kbps, sample_rate,
    bit_depth, channels, codec, file_size, date_added, date_modified, play_count, last_played,
    skip_count, rating, kind, compilation, sort_title, sort_artist, sort_album, sort_album_artist,
    enabled";

impl Library {
    pub fn tracks(&self, query: &TrackQuery) -> Result<Vec<Track>> {
        let selection = selection(&self.conn, query, true)?;
        let sql = format!(
            "SELECT {TRACK_COLUMNS} FROM {} {} ORDER BY {}",
            selection.from,
            selection.where_sql(),
            order_by(query.sort, query.descending, selection.lists_items)
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(selection.params), track_from_row)?;
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
        let selection = selection(&self.conn, query, true)?;
        let sql = format!(
            "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0), COALESCE(SUM(file_size), 0)
             FROM {} {}",
            selection.from,
            selection.where_sql()
        );
        Ok(self.conn.prepare_cached(&sql)?.query_row(
            params_from_iter(selection.params),
            |row| {
                Ok(LibraryStats {
                    count: row.get(0)?,
                    total_duration_ms: row.get(1)?,
                    total_bytes: row.get(2)?,
                })
            },
        )?)
    }
}

/// Positional access: by-name lookup rescans the column list per field, which
/// dominates the cost of materialising a 100k-row library.
pub(crate) fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
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
        enabled: row.get(32)?,
    })
}

/// The rows a query ranges over: its `FROM`, filters and bound parameters.
pub(crate) struct Selection {
    pub from: String,
    clauses: Vec<String>,
    pub params: Vec<Box<dyn ToSql>>,
    /// Rows come from a playlist's stored items, so `pi.position` can order them.
    lists_items: bool,
}

impl Selection {
    pub(crate) fn where_sql(&self) -> String {
        if self.clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", self.clauses.join(" AND "))
        }
    }

    pub(crate) fn require(&mut self, clause: &str) {
        self.clauses.push(clause.to_owned());
    }
}

/// Everything user-supplied is bound as a parameter. `instr` avoids LIKE's
/// wildcard escaping, and `rarray` carries the id list as a single parameter.
/// `keep_duplicates` is for listings, where a playlist shows a track once per
/// entry; aggregations pass false so each track counts once.
pub(crate) fn selection(
    conn: &Connection,
    query: &TrackQuery,
    keep_duplicates: bool,
) -> Result<Selection> {
    let mut selection = Selection {
        from: "tracks".to_owned(),
        clauses: Vec::new(),
        params: Vec::new(),
        lists_items: false,
    };
    if let Some(id) = query.playlist {
        let row = playlist_row(conn, id)?.ok_or(Error::PlaylistNotFound(id))?;
        if !row.holds_items()? {
            let mut compile = Compile::new(now_unix(), true);
            let members = members_sql(conn, &row, &mut compile)?;
            selection.clauses.push(format!("id IN ({members})"));
            selection.params.append(&mut compile.params);
        } else if keep_duplicates {
            selection.from =
                "playlist_items AS pi JOIN tracks ON tracks.id = pi.track_id".to_owned();
            selection.clauses.push("pi.playlist_id = ?".to_owned());
            selection.params.push(Box::new(id));
            selection.lists_items = true;
        } else {
            selection.clauses.push(
                "id IN (SELECT track_id FROM playlist_items WHERE playlist_id = ?)".to_owned(),
            );
            selection.params.push(Box::new(id));
        }
    }
    if let Some(kind) = query.kind {
        selection.clauses.push("kind = ?".to_owned());
        selection.params.push(Box::new(kind));
    }
    if let Some(search) = &query.search {
        for word in fold(search).split_whitespace() {
            selection
                .clauses
                .push("instr(search_text, ?) > 0".to_owned());
            selection.params.push(Box::new(word.to_owned()));
        }
    }
    if let Some(ids) = &query.ids {
        selection
            .clauses
            .push("id IN (SELECT value FROM rarray(?))".to_owned());
        let values: Vec<Value> = ids.iter().copied().map(Value::Integer).collect();
        selection.params.push(Box::new(Rc::new(values)));
    }
    Ok(selection)
}

/// Only the primary key honours `descending`, matching iTunes; the tie-break
/// columns always ascend. Column names come from this closed set, never from
/// user input, so interpolating them into SQL is safe.
fn order_by(sort: SortKey, descending: bool, lists_items: bool) -> String {
    // Outside a playlist's own listing there is no playlist order to follow.
    let sort = match sort {
        SortKey::PlaylistOrder if !lists_items => SortKey::Artist,
        other => other,
    };
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
        SortKey::PlaylistOrder => ("pi.position", &[]),
    };
    let direction = if descending { "DESC" } else { "ASC" };
    let mut clauses = vec![format!("{primary} {direction} NULLS LAST")];
    clauses.extend(tail.iter().map(|column| format!("{column} ASC NULLS LAST")));
    clauses.push("id ASC".to_owned());
    clauses.join(", ")
}
