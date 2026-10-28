use rusqlite::{Row, params_from_iter};

use crate::error::Result;
use crate::library::Library;
use crate::model::{
    AlbumSummary, ArtistSummary, BrowserFacets, ComposerSummary, GenreSummary, SortKey, Track,
    TrackQuery,
};
use crate::normalize::COMPILATIONS_GROUP;
use crate::query::{TRACK_COLUMNS, selection, track_from_row};

/// The name a group displays under: all its rows share one folded artist, so
/// `MIN` just picks one spelling of it.
fn artist_name() -> String {
    format!(
        "CASE WHEN artist_group = '{COMPILATIONS_GROUP}' THEN 'Compilations'
         ELSE MIN(CASE WHEN album_artist = '' THEN artist ELSE album_artist END) END"
    )
}

/// Compilations sort as "Compilations" whatever their tracks' own artists are.
fn artist_sort() -> String {
    format!(
        "CASE WHEN artist_group = '{COMPILATIONS_GROUP}' THEN '{COMPILATIONS_GROUP}'
         ELSE MIN(key_album_artist) END"
    )
}

fn album_sort() -> &'static str {
    "MIN(key_album)"
}

fn album_columns() -> String {
    format!(
        "album_group, MIN(album), {}, MIN(year), COALESCE(MIN(NULLIF(genre, '')), ''),
         COUNT(*), SUM(duration_ms), MIN(id), MAX(date_added), MAX(compilation)",
        artist_name()
    )
}

fn album_from_row(row: &Row<'_>) -> rusqlite::Result<AlbumSummary> {
    Ok(AlbumSummary {
        key: row.get(0)?,
        album: row.get(1)?,
        album_artist: row.get(2)?,
        year: row.get(3)?,
        genre: row.get(4)?,
        track_count: row.get(5)?,
        duration_ms: row.get(6)?,
        artwork_track_id: row.get(7)?,
        date_added: row.get(8)?,
        compilation: row.get(9)?,
    })
}

fn direction(descending: bool) -> &'static str {
    if descending { "DESC" } else { "ASC" }
}

impl Library {
    /// Groups by album artist (the track artist when there is none) and album.
    /// Sorted by album, or by artist then album when the query sorts by artist;
    /// `descending` flips the first key only, as for tracks.
    pub fn albums(&self, query: &TrackQuery) -> Result<Vec<AlbumSummary>> {
        let selection = selection(&self.conn, query, false)?;
        let dir = direction(query.descending);
        let order = match query.sort {
            SortKey::Year => format!(
                "MIN(year) {dir} NULLS LAST, {} ASC NULLS LAST",
                album_sort()
            ),
            SortKey::Artist | SortKey::AlbumArtist => format!(
                "{} {dir} NULLS LAST, {} ASC NULLS LAST",
                artist_sort(),
                album_sort()
            ),
            _ => format!(
                "{} {dir} NULLS LAST, {} ASC NULLS LAST",
                album_sort(),
                artist_sort()
            ),
        };
        let sql = format!(
            "SELECT {} FROM {} {} GROUP BY album_group ORDER BY {order}, album_group",
            album_columns(),
            selection.from,
            selection.where_sql()
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(selection.params), album_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every track of the album, whatever its media kind, in disc and track order.
    pub fn album_tracks(&self, key: &str) -> Result<Vec<Track>> {
        let sql = format!(
            "SELECT {TRACK_COLUMNS} FROM tracks WHERE album_group = ?1
             ORDER BY disc_number ASC NULLS LAST, track_number ASC NULLS LAST,
                      key_title ASC NULLS LAST, id ASC"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([key], track_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn artists(&self, query: &TrackQuery) -> Result<Vec<ArtistSummary>> {
        self.artist_summaries(query, true)
    }

    fn artist_summaries(
        &self,
        query: &TrackQuery,
        with_artwork: bool,
    ) -> Result<Vec<ArtistSummary>> {
        let selection = selection(&self.conn, query, false)?;
        let sql = format!(
            "SELECT artist_group, {}, COUNT(DISTINCT album_group), COUNT(*)
             FROM {} {} GROUP BY artist_group
             ORDER BY {} {} NULLS LAST, artist_group",
            artist_name(),
            selection.from,
            selection.where_sql(),
            artist_sort(),
            direction(query.descending)
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(
            params_from_iter(selection.params.iter().map(|p| p.as_ref())),
            |row| {
                Ok(ArtistSummary {
                    key: row.get(0)?,
                    name: row.get(1)?,
                    album_count: row.get(2)?,
                    track_count: row.get(3)?,
                    artwork_track_ids: Vec::new(),
                })
            },
        )?;
        let mut artists = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        if !with_artwork {
            return Ok(artists);
        }
        // One bounded result per artist, rather than a query (or file read) per tile.
        let sql = format!(
            "WITH albums AS (
                SELECT artist_group, album_group, MIN(id) AS cover FROM {} {} GROUP BY artist_group, album_group
             ), ranked AS (
                SELECT artist_group, cover, ROW_NUMBER() OVER (PARTITION BY artist_group ORDER BY album_group) AS n FROM albums
             ) SELECT artist_group, GROUP_CONCAT(cover) FROM ranked WHERE n <= 4 GROUP BY artist_group",
            selection.from, selection.where_sql()
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let covers = stmt
            .query_map(
                params_from_iter(selection.params.iter().map(|p| p.as_ref())),
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()?;
        for artist in &mut artists {
            if let Some(ids) = covers.get(&artist.key) {
                artist.artwork_track_ids =
                    ids.split(',').filter_map(|id| id.parse().ok()).collect();
            }
        }
        Ok(artists)
    }

    /// Oldest first, as iTunes lists an artist's albums.
    pub fn artist_albums(&self, key: &str) -> Result<Vec<AlbumSummary>> {
        self.artist_albums_in(
            key,
            &TrackQuery {
                kind: None,
                ..TrackQuery::default()
            },
        )
    }

    /// The same chronological navigation constrained by search, playlist, kind and browser filters.
    pub fn artist_albums_in(&self, key: &str, query: &TrackQuery) -> Result<Vec<AlbumSummary>> {
        self.albums(&TrackQuery {
            artist_group: Some(key.to_owned()),
            sort: SortKey::Year,
            descending: false,
            ..query.clone()
        })
    }

    /// Each successive pane uses only the choices to its left. Search and source scope stay intact.
    pub fn browser_facets(&self, query: &TrackQuery) -> Result<BrowserFacets> {
        let genres = self.genres(&TrackQuery {
            genre: None,
            artist_group: None,
            album_group: None,
            descending: false,
            ..query.clone()
        })?;
        let artists = self.artist_summaries(
            &TrackQuery {
                artist_group: None,
                album_group: None,
                descending: false,
                ..query.clone()
            },
            false,
        )?;
        let albums = self.albums(&TrackQuery {
            album_group: None,
            sort: SortKey::Album,
            descending: false,
            ..query.clone()
        })?;
        Ok(BrowserFacets {
            genres,
            artists,
            albums,
        })
    }

    /// Songs without a genre are left out: a blank entry is no use as a tab.
    pub fn genres(&self, query: &TrackQuery) -> Result<Vec<GenreSummary>> {
        let mut selection = selection(&self.conn, query, false)?;
        selection.require("key_genre IS NOT NULL");
        let sql = format!(
            "SELECT MIN(genre), COUNT(DISTINCT album_group), COUNT(*) FROM {} {}
             GROUP BY key_genre ORDER BY key_genre {}",
            selection.from,
            selection.where_sql(),
            direction(query.descending)
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(selection.params), |row| {
            Ok(GenreSummary {
                name: row.get(0)?,
                album_count: row.get(1)?,
                track_count: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Songs without a composer are left out, like genres.
    pub fn composers(&self, query: &TrackQuery) -> Result<Vec<ComposerSummary>> {
        let mut selection = selection(&self.conn, query, false)?;
        selection.require("composer <> ''");
        let sql = format!(
            "SELECT MIN(composer), COUNT(*) FROM {} {} GROUP BY fold(composer)
             ORDER BY fold(composer) {}",
            selection.from,
            selection.where_sql(),
            direction(query.descending)
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(selection.params), |row| {
            Ok(ComposerSummary {
                name: row.get(0)?,
                track_count: row.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
