use crate::labels;
use crate::model::Song;

/// What a playlist CSV turned out to hold.
pub(crate) enum Table {
    Songs(Vec<Song>),
    /// Only video ids, as Google's playlist files are.
    VideoIds(Vec<String>),
}

const VIDEO_ID: &str = "video id";

/// Where a playlist CSV keeps what is wanted of it.
struct Columns {
    title: usize,
    artists: Vec<usize>,
    album: Option<usize>,
    duration: Option<usize>,
    id: Option<usize>,
}

impl Columns {
    /// Found by header, since Exportify has renamed its columns over the years and translates them,
    /// and Google splits a song's artists across several.
    fn from_header(header: &[String]) -> Option<Self> {
        let find = |names: &[&str]| {
            header
                .iter()
                .position(|name| names.contains(&name.as_str()))
        };
        let artists: Vec<usize> = header
            .iter()
            .enumerate()
            .filter(|(_, name)| labels::ARTIST.contains(&name.as_str()) || numbered_artist(name))
            .map(|(column, _)| column)
            .collect();
        Some(Columns {
            title: find(labels::TITLE)?,
            artists: Some(artists).filter(|artists| !artists.is_empty())?,
            album: find(labels::ALBUM),
            duration: find(labels::DURATION),
            id: find(labels::ID),
        })
    }

    /// Exportify's columns in the order it has always written them, for a file whose labels are in
    /// a language this doesn't know: its rows begin with a `spotify:track:` URI (though a local
    /// file's begins `spotify:local:`, so the first row may not).
    fn exportify(rows: &[Vec<String>]) -> Option<Self> {
        rows.iter()
            .any(|row| {
                row.first()
                    .is_some_and(|uri| uri.starts_with("spotify:track:"))
            })
            .then(|| Columns {
                title: 1,
                artists: vec![3],
                album: Some(5),
                duration: Some(12),
                id: Some(0),
            })
    }

    fn song(&self, row: &[String]) -> Option<Song> {
        let artist = self
            .artists
            .iter()
            .filter_map(|column| non_empty(row.get(*column)?))
            .collect::<Vec<_>>()
            .join(", ");
        Some(Song {
            title: non_empty(row.get(self.title)?)?,
            // Exportify separates artists with ", " (escaping commas inside a name) and used to
            // use ";"; the matcher splits on ", ".
            artist: non_empty(&artist.replace(';', ", ").replace("\\,", ","))?,
            album: self.album.and_then(|column| non_empty(row.get(column)?)),
            duration_ms: self
                .duration
                .and_then(|column| row.get(column)?.trim().parse().ok())
                .filter(|ms| *ms > 0),
            service_id: self.id.and_then(|column| non_empty(row.get(column)?)),
        })
    }
}

/// Reads a playlist CSV whatever tool wrote it. `None` means the file isn't a list of songs at all
/// (a playlists index, a list of channels).
pub(crate) fn read(text: &str) -> Option<Table> {
    let mut records = records(text);
    if records.is_empty() {
        return None;
    }
    let header: Vec<String> = records
        .remove(0)
        .into_iter()
        .map(|name| name.trim().to_lowercase())
        .collect();
    let Some(columns) = Columns::from_header(&header).or_else(|| Columns::exportify(&records))
    else {
        let video = header.iter().position(|name| name == VIDEO_ID)?;
        return Some(Table::VideoIds(
            records
                .iter()
                .filter_map(|row| non_empty(row.get(video)?))
                .collect(),
        ));
    };
    Some(Table::Songs(
        records.iter().filter_map(|row| columns.song(row)).collect(),
    ))
}

/// "artist name 1" .. "artist name 4", how Google lists a song's artists.
fn numbered_artist(name: &str) -> bool {
    name.strip_prefix("artist name ")
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Splits RFC 4180 text into records: a field may be quoted, quotes are doubled inside one, and a
/// quoted field may hold commas and line breaks. Blank lines are dropped.
fn records(text: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .chars()
        .peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => record.push(std::mem::take(&mut field)),
            '\r' if !quoted => {}
            '\n' if !quoted => end_record(&mut records, &mut record, &mut field),
            _ => field.push(c),
        }
    }
    end_record(&mut records, &mut record, &mut field);
    records
}

fn end_record(records: &mut Vec<Vec<String>>, record: &mut Vec<String>, field: &mut String) {
    if record.is_empty() && field.is_empty() {
        return;
    }
    record.push(std::mem::take(field));
    records.push(std::mem::take(record));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn songs(text: &str) -> Vec<Song> {
        match read(text) {
            Some(Table::Songs(songs)) => songs,
            _ => panic!("not a list of songs"),
        }
    }

    #[test]
    fn quotes_commas_and_line_breaks_stay_inside_their_field() {
        let text = "\u{feff}a,b\r\n\"x, \"\"y\"\"\",\"two\nlines\"\r\n\r\nlast,";
        assert_eq!(
            records(text),
            vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec!["x, \"y\"".to_owned(), "two\nlines".to_owned()],
                vec!["last".to_owned(), String::new()],
            ]
        );
    }

    #[test]
    fn exportify_columns_are_found_by_their_headers() {
        let text = "\"Track URI\",\"Track Name\",\"Artist Name(s)\",\"Album Name\",\"Track Duration (ms)\"\n\
                    \"spotify:track:1\",\"Song\",\"Tyler\\, The Creator, Guest\",\"Album\",\"201000\"\n\
                    \"spotify:track:2\",\"Other\",\"Old;Style\",\"\",\"0\"\n";
        assert_eq!(
            songs(text),
            vec![
                Song {
                    title: "Song".into(),
                    artist: "Tyler, The Creator, Guest".into(),
                    album: Some("Album".into()),
                    duration_ms: Some(201_000),
                    service_id: Some("spotify:track:1".into()),
                },
                Song {
                    title: "Other".into(),
                    artist: "Old, Style".into(),
                    album: None,
                    duration_ms: None,
                    service_id: Some("spotify:track:2".into()),
                },
            ]
        );
    }

    #[test]
    fn google_splits_artists_across_numbered_columns() {
        let text = "Video ID,Song Title,Album Title,Artist Name 1,Artist Name 2,Artist Name 3\nabc,Song,Album,One,Two,\n";
        let found = songs(text);
        assert_eq!(found[0].artist, "One, Two");
        assert_eq!(found[0].service_id.as_deref(), Some("abc"));
    }

    #[test]
    fn a_file_of_video_ids_is_reported_as_such() {
        let Some(Table::VideoIds(ids)) =
            read("Video ID,Playlist video creation timestamp\nabc,2026-02-12T10:15:52+00:00\n,x\n")
        else {
            panic!("not video ids");
        };
        assert_eq!(ids, ["abc"]);
    }

    #[test]
    fn files_that_are_not_songs_are_ignored() {
        assert!(read("Playlist ID,Playlist title (original)\nPL1,Road Trip\n").is_none());
        assert!(read("Channel ID,Channel URL,Channel title\nUC1,http://x,Y\n").is_none());
        assert!(read("").is_none());
    }

    #[test]
    fn rows_without_a_title_or_artist_are_skipped() {
        assert_eq!(songs("Title,Artist\n,Someone\nSong,\nSong,Band\n").len(), 1);
    }
}
