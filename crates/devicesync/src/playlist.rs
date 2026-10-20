//! Writing playlists in the formats players read.

use std::collections::HashMap;

use crate::{PlaylistFormat, SyncItem};

/// One playlist line: where the file is and what to call it.
pub(crate) struct Line<'a> {
    pub path: &'a str,
    pub item: &'a SyncItem,
}

/// The playlist as the bytes to write. Paths are relative to `playlist_dir`, so the player finds
/// the music from wherever it opens the playlist.
pub(crate) fn render(format: PlaylistFormat, playlist_dir: &str, lines: &[Line<'_>]) -> Vec<u8> {
    let entries: Vec<(String, String)> = lines
        .iter()
        .map(|line| (relative(playlist_dir, line.path), label(line.item)))
        .collect();
    match format {
        PlaylistFormat::M3u8 => m3u8(&entries),
        PlaylistFormat::Pls => pls(&entries),
    }
}

/// CRLF line ends, which every player's parser copes with.
const EOL: &str = "\r\n";

fn m3u8(entries: &[(String, String)]) -> Vec<u8> {
    let mut text = format!("#EXTM3U{EOL}");
    for (path, label) in entries {
        // The length is unknown (-1), which the format allows.
        text.push_str(&format!("#EXTINF:-1,{label}{EOL}{path}{EOL}"));
    }
    text.into_bytes()
}

fn pls(entries: &[(String, String)]) -> Vec<u8> {
    let mut text = format!("[playlist]{EOL}");
    for (number, (path, label)) in (1..).zip(entries) {
        text.push_str(&format!(
            "File{number}={path}{EOL}Title{number}={label}{EOL}Length{number}=-1{EOL}"
        ));
    }
    text.push_str(&format!(
        "NumberOfEntries={}{EOL}Version=2{EOL}",
        entries.len()
    ));
    text.into_bytes()
}

/// "Artist - Title" on one line.
fn label(item: &SyncItem) -> String {
    let flat = |text: &str| {
        text.split(['\r', '\n'])
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_owned()
    };
    let (artist, title) = (flat(&item.album_artist), flat(&item.title));
    if artist.is_empty() {
        title
    } else {
        format!("{artist} - {title}")
    }
}

/// `to` as a path relative to the folder `from`: `Music/A/x.flac` from `Playlists` is
/// `../Music/A/x.flac`.
pub(crate) fn relative(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|part| !part.is_empty()).collect();
    let to: Vec<&str> = to.split('/').filter(|part| !part.is_empty()).collect();
    let shared = from
        .iter()
        .zip(&to)
        .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
        .count();
    let mut parts = vec![".."; from.len() - shared];
    parts.extend(&to[shared..]);
    parts.join("/")
}

/// The lines for `ids` in order, skipping any that are not on the device or not in `items`.
pub(crate) fn lines<'a>(
    ids: &[i64],
    on_device: &'a HashMap<i64, &'a str>,
    items: &'a HashMap<i64, &'a SyncItem>,
) -> Vec<Line<'a>> {
    ids.iter()
        .filter_map(|id| {
            Some(Line {
                path: on_device.get(id)?,
                item: items.get(id)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, artist: &str, title: &str) -> SyncItem {
        SyncItem {
            library_id: id,
            source: "/x".into(),
            album_artist: artist.to_owned(),
            album: "Album".to_owned(),
            title: title.to_owned(),
            disc: None,
            track: None,
            size: 1,
            mtime: 1,
        }
    }

    #[test]
    fn relative_paths_climb_out_of_the_playlist_folder() {
        assert_eq!(relative("Playlists", "Music/A/x.flac"), "../Music/A/x.flac");
        assert_eq!(relative("Music/Playlists", "Music/A/x.flac"), "../A/x.flac");
        assert_eq!(relative("Music", "Music/A/x.flac"), "A/x.flac");
        assert_eq!(
            relative("Audio/Lists/Deep", "Music/x.flac"),
            "../../../Music/x.flac"
        );
        assert_eq!(relative("playlists", "PLAYLISTS/x.flac"), "x.flac");
        assert_eq!(relative("Music", "Music/x.flac"), "x.flac");
    }

    #[test]
    fn m3u8_has_header_extinf_and_relative_paths() {
        let a = item(1, "The Beatles", "Come Together");
        let b = item(2, "", "Untitled Jam");
        let lines = [
            Line {
                path: "Music/The Beatles/Abbey Road/1-01 Come Together.flac",
                item: &a,
            },
            Line {
                path: "Music/Jam.flac",
                item: &b,
            },
        ];
        let text = String::from_utf8(render(PlaylistFormat::M3u8, "Playlists", &lines)).unwrap();
        assert_eq!(
            text,
            "#EXTM3U\r\n#EXTINF:-1,The Beatles - Come Together\r\n../Music/The Beatles/Abbey Road/1-01 Come Together.flac\r\n\
             #EXTINF:-1,Untitled Jam\r\n../Music/Jam.flac\r\n"
        );
    }

    #[test]
    fn utf8_survives_and_newlines_in_tags_cannot_break_lines() {
        let a = item(1, "Sigur R\u{f3}s", "Hopp\u{ed}polla\nbonus\r\ntrack");
        let text = String::from_utf8(render(
            PlaylistFormat::M3u8,
            "Playlists",
            &[Line {
                path: "Music/\u{307f}.flac",
                item: &a,
            }],
        ))
        .unwrap();
        assert!(text.contains("#EXTINF:-1,Sigur R\u{f3}s - Hopp\u{ed}polla bonus  track\r\n../Music/\u{307f}.flac\r\n"), "{text}");
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn pls_numbers_its_entries() {
        let a = item(1, "A", "One");
        let b = item(2, "B", "Two");
        let lines = [
            Line {
                path: "Music/1.mp3",
                item: &a,
            },
            Line {
                path: "Music/2.mp3",
                item: &b,
            },
        ];
        let text = String::from_utf8(render(PlaylistFormat::Pls, "Playlists", &lines)).unwrap();
        assert_eq!(
            text,
            "[playlist]\r\nFile1=../Music/1.mp3\r\nTitle1=A - One\r\nLength1=-1\r\nFile2=../Music/2.mp3\r\nTitle2=B - Two\r\nLength2=-1\r\n\
             NumberOfEntries=2\r\nVersion=2\r\n"
        );
    }

    #[test]
    fn an_empty_playlist_is_still_a_valid_file() {
        assert_eq!(
            render(PlaylistFormat::M3u8, "Playlists", &[]),
            b"#EXTM3U\r\n"
        );
        let pls = String::from_utf8(render(PlaylistFormat::Pls, "Playlists", &[])).unwrap();
        assert!(pls.contains("NumberOfEntries=0"));
    }

    #[test]
    fn lines_follow_playlist_order_and_skip_what_is_not_on_the_device() {
        let (a, b, c) = (item(1, "A", "a"), item(2, "B", "b"), item(3, "C", "c"));
        let items: HashMap<i64, &SyncItem> = [(1, &a), (2, &b), (3, &c)].into();
        let on_device: HashMap<i64, &str> = [(1, "Music/a.flac"), (3, "Music/c.flac")].into();
        let found = lines(&[3, 2, 1, 99], &on_device, &items);
        assert_eq!(
            found.iter().map(|line| line.path).collect::<Vec<_>>(),
            ["Music/c.flac", "Music/a.flac"]
        );
    }
}
