use rusqlite::{Connection, named_params};

use crate::error::Result;
use crate::model::Track;
use crate::normalize::{fold, name_key, plain_key};
use crate::tags::TAG_PARSER_VERSION;

const COLUMNS: &[&str] = &[
    "id",
    "path",
    "title",
    "artist",
    "album_artist",
    "album",
    "genre",
    "composer",
    "year",
    "track_number",
    "track_count",
    "disc_number",
    "disc_count",
    "duration_ms",
    "bitrate_kbps",
    "sample_rate",
    "bit_depth",
    "channels",
    "codec",
    "file_size",
    "date_added",
    "date_modified",
    "play_count",
    "last_played",
    "skip_count",
    "rating",
    "kind",
    "compilation",
    "sort_title",
    "sort_artist",
    "sort_album",
    "sort_album_artist",
    "key_title",
    "key_artist",
    "key_album",
    "key_album_artist",
    "key_genre",
    "search_text",
    "parser_version",
];

/// What a rescan must not clobber: row identity and everything the user accumulates.
const KEPT_ON_REFRESH: &[&str] = &[
    "id",
    "path",
    "date_added",
    "play_count",
    "last_played",
    "skip_count",
    "rating",
];

pub(crate) enum WriteMode {
    /// Rows come from re-reading files: SQLite assigns new ids, and a known
    /// path keeps its id and user state while its tag-derived columns refresh.
    RefreshTags,
    /// Rows come from another database: the caller's ids are authoritative and
    /// every column is overwritten. A path already held by a different id is a
    /// constraint error rather than a silent deletion of that row.
    Replace,
}

/// The single place rows are written, so scanned and imported tracks get
/// identical derived sort and search columns. One transaction per call.
pub(crate) fn write_tracks<'a>(
    conn: &mut Connection,
    mode: WriteMode,
    tracks: impl IntoIterator<Item = &'a Track>,
) -> Result<()> {
    let (conflict_target, kept) = match mode {
        WriteMode::RefreshTags => ("path", KEPT_ON_REFRESH),
        WriteMode::Replace => ("id", &["id"][..]),
    };
    let placeholders = COLUMNS
        .iter()
        .map(|column| format!(":{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let updates = COLUMNS
        .iter()
        .filter(|column| !kept.contains(*column))
        .map(|column| format!("{column} = excluded.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO tracks ({}) VALUES ({placeholders})
         ON CONFLICT({conflict_target}) DO UPDATE SET {updates}",
        COLUMNS.join(", ")
    );

    let tx = conn.transaction()?;
    {
        let mut upsert = tx.prepare_cached(&sql)?;
        for track in tracks {
            // An untagged album artist sorts under the track artist, as in iTunes.
            let key_album_artist = if track.album_artist.is_empty() {
                name_key(track.sort_artist.as_deref(), &track.artist)
            } else {
                name_key(track.sort_album_artist.as_deref(), &track.album_artist)
            };
            let search_text = fold(
                &[
                    &track.title,
                    &track.artist,
                    &track.album_artist,
                    &track.album,
                    &track.composer,
                    &track.genre,
                ]
                .map(String::as_str)
                .join(" "),
            );
            let id = match mode {
                WriteMode::RefreshTags => None,
                WriteMode::Replace => Some(track.id),
            };
            upsert.execute(named_params! {
                ":id": id,
                ":path": track.path,
                ":title": track.title,
                ":artist": track.artist,
                ":album_artist": track.album_artist,
                ":album": track.album,
                ":genre": track.genre,
                ":composer": track.composer,
                ":year": track.year,
                ":track_number": track.track_number,
                ":track_count": track.track_count,
                ":disc_number": track.disc_number,
                ":disc_count": track.disc_count,
                ":duration_ms": track.duration_ms,
                ":bitrate_kbps": track.bitrate_kbps,
                ":sample_rate": track.sample_rate,
                ":bit_depth": track.bit_depth,
                ":channels": track.channels,
                ":codec": track.codec,
                ":file_size": track.file_size,
                ":date_added": track.date_added,
                ":date_modified": track.date_modified,
                ":play_count": track.play_count,
                ":last_played": track.last_played,
                ":skip_count": track.skip_count,
                ":rating": track.rating,
                ":kind": track.kind,
                ":compilation": track.compilation,
                ":sort_title": track.sort_title,
                ":sort_artist": track.sort_artist,
                ":sort_album": track.sort_album,
                ":sort_album_artist": track.sort_album_artist,
                ":key_title": name_key(track.sort_title.as_deref(), &track.title),
                ":key_artist": name_key(track.sort_artist.as_deref(), &track.artist),
                ":key_album": name_key(track.sort_album.as_deref(), &track.album),
                ":key_album_artist": key_album_artist,
                ":key_genre": plain_key(&track.genre),
                ":search_text": search_text,
                // Imported rows have no file to re-read, so they count as current.
                ":parser_version": TAG_PARSER_VERSION,
            })?;
        }
    }
    tx.commit()?;
    Ok(())
}
