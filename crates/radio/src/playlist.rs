//! The playlist files stations are published as, and finding out what an address really is.
//!
//! A station's address is often not a stream but a `.pls` or `.m3u` that names the stream (or
//! several mirrors of it), and sometimes an HLS playlist, which looks like an `.m3u8` but is
//! itself the stream.

use std::io::{Cursor, Read};

use url::Url;

use crate::{
    error::{Error, Result},
    http::{CancelHandle, Headers, Http, HttpConfig, Response, parse_url},
    live::format,
    text::decode,
};

/// Playlists are tens of lines; a megabyte is a mistake.
const MAX_PLAYLIST_BYTES: u64 = 1 << 20;
/// How much of an unlabelled body is looked at to decide whether it is a playlist.
const PEEK_BYTES: u64 = 1024;

const PLAYLIST_TYPES: [&str; 9] = [
    "audio/x-scpls",
    "audio/scpls",
    "application/pls+xml",
    "audio/mpegurl",
    "audio/x-mpegurl",
    "audio/m3u",
    "application/x-mpegurl",
    "application/vnd.apple.mpegurl",
    "application/vnd.apple.mpegurl.audio",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playlist {
    /// Stream addresses, in the order the file lists them.
    Entries(Vec<Url>),
    /// An HLS playlist. It is not a list of streams: the address it was read from is the stream.
    Hls,
}

/// Reads a playlist document. `base` is where it came from, for entries written as relative paths.
///
/// PLS (`File1=...`), M3U and extended M3U (`#EXTINF` lines, then addresses) are understood, along
/// with a bare list of addresses. Anything that is not http or https (local paths in an M3U that
/// escaped from someone's disk) is dropped, as are repeats.
pub fn parse(text: &str, base: &Url) -> Playlist {
    let text = text.trim_start_matches('\u{feff}');
    // `#EXTM3U` alone is a plain M3U; the `#EXT-X-` tags are what HLS adds.
    if text
        .lines()
        .any(|line| line.trim_start().starts_with("#EXT-X-"))
    {
        return Playlist::Hls;
    }
    let mut entries: Vec<Url> = Vec::new();
    let candidates = if is_pls(text) {
        pls_files(text)
    } else {
        m3u_lines(text)
    };
    for candidate in candidates
        .into_iter()
        .filter(|candidate| !candidate.contains(['<', '>']))
    {
        let Ok(url) = base.join(candidate.trim()) else {
            continue;
        };
        if matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && !entries.contains(&url)
        {
            entries.push(url);
        }
    }
    Playlist::Entries(entries)
}

fn is_pls(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim().eq_ignore_ascii_case("[playlist]") || pls_file(line).is_some())
}

/// `File3=http://...` as (3, "http://..."). The key is case-insensitive in the wild.
fn pls_file(line: &str) -> Option<(u32, &str)> {
    let (key, value) = line.trim().split_once('=')?;
    let number = key
        .trim()
        .get(..4)
        .filter(|prefix| prefix.eq_ignore_ascii_case("file"))
        .and_then(|_| key.trim()[4..].parse().ok())?;
    Some((number, value))
}

/// The `FileN` values in the order of N, which is not always the order of the lines.
fn pls_files(text: &str) -> Vec<&str> {
    let mut files: Vec<(u32, &str)> = text.lines().filter_map(pls_file).collect();
    files.sort_by_key(|&(number, _)| number);
    files.into_iter().map(|(_, value)| value).collect()
}

fn m3u_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// Whether the start of a body is a playlist document, for servers that do not label one.
fn looks_like_playlist(head: &[u8]) -> bool {
    let text = decode(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    let lower = text
        .get(..text.len().min(16))
        .unwrap_or_default()
        .to_ascii_lowercase();
    lower.starts_with("#extm3u")
        || lower.starts_with("#ext-x-")
        || lower.starts_with("[playlist]")
        // A bare list of addresses, and not audio that happens to start with those letters.
        || ((lower.starts_with("http://") || lower.starts_with("https://")) && !head.iter().any(|&byte| byte < 9))
}

pub(crate) enum Probed {
    Playlist {
        playlist: Playlist,
        text: String,
        base: Url,
    },
    /// A response whose body is audio, positioned at its start.
    Audio(Response),
}

/// Fetches `url` and works out whether it is a playlist or a stream, from the content type where
/// that is trustworthy and the body where it is not. `icy` asks for in-band metadata.
pub(crate) fn probe(http: &Http, url: &Url, icy: bool) -> Result<Probed> {
    let request_headers: &[(&str, &str)] = if icy { &[("Icy-MetaData", "1")] } else { &[] };
    let mut response = http.get(url, request_headers)?;
    let media_type = response.headers.media_type();
    let declared_playlist = media_type
        .as_deref()
        .is_some_and(|media| PLAYLIST_TYPES.contains(&media));
    if !declared_playlist && says_audio(&response.headers, media_type.as_deref()) {
        return Ok(Probed::Audio(response));
    }

    let mut head = Vec::new();
    (&mut response.body)
        .take(PEEK_BYTES)
        .read_to_end(&mut head)
        .map_err(|error| crate::http::io_error(url, &error))?;
    if declared_playlist || looks_like_playlist(&head) {
        let mut rest = Vec::new();
        (&mut response.body)
            .take(MAX_PLAYLIST_BYTES)
            .read_to_end(&mut rest)
            .map_err(|error| crate::http::io_error(url, &error))?;
        head.extend(rest);
        let text = decode(&head);
        let base = response.url;
        return Ok(Probed::Playlist {
            playlist: parse(&text, &base),
            text,
            base,
        });
    }
    // Pages and documents are not streams; anything else is, and the decoder can say which.
    if media_type.as_deref().is_some_and(|media| {
        media.starts_with("text/") || media.contains("html") || media.contains("json")
    }) {
        return Err(Error::NotAStream(url.to_string()));
    }
    response.body = Box::new(Cursor::new(head).chain(response.body));
    Ok(Probed::Audio(response))
}

fn says_audio(headers: &Headers, media_type: Option<&str>) -> bool {
    headers.get("icy-metaint").is_some()
        || headers.get("icy-name").is_some()
        || headers.get("icy-br").is_some()
        || media_type.is_some_and(|media| {
            media.starts_with("audio/") || format::from_media_type(media).is_some()
        })
}

/// The stream addresses `url` leads to: the addresses a `.pls` or `.m3u` lists, or `url` itself if
/// it already is a stream (including an HLS one). The entries are not themselves fetched, so one
/// that is dead is not noticed until it is opened.
pub fn resolve(url: &str) -> Result<Vec<Url>> {
    resolve_with(url, &HttpConfig::default())
}

pub fn resolve_with(url: &str, config: &HttpConfig) -> Result<Vec<Url>> {
    let url = parse_url(url)?;
    let http = Http::new(config.clone(), CancelHandle::new());
    match probe(&http, &url, false)? {
        Probed::Audio(_)
        | Probed::Playlist {
            playlist: Playlist::Hls,
            ..
        } => Ok(vec![url]),
        Probed::Playlist {
            playlist: Playlist::Entries(entries),
            ..
        } if entries.is_empty() => Err(Error::NotAStream(url.to_string())),
        Probed::Playlist {
            playlist: Playlist::Entries(entries),
            ..
        } => Ok(entries),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("http://radio.example.com/listen/station.pls").unwrap()
    }

    fn entries(text: &str) -> Vec<String> {
        match parse(text, &base()) {
            Playlist::Entries(entries) => entries.into_iter().map(String::from).collect(),
            Playlist::Hls => panic!("parsed as HLS"),
        }
    }

    #[test]
    fn pls_files_are_read_in_numeric_order() {
        let text = "[playlist]\nNumberOfEntries=3\nFile2=http://b.example.com:8000/two\nTitle2=Two\nLength2=-1\nFile1=http://a.example.com/one\nFile10=http://c.example.com/ten\nVersion=2\n";
        assert_eq!(
            entries(text),
            [
                "http://a.example.com/one",
                "http://b.example.com:8000/two",
                "http://c.example.com/ten"
            ]
        );
    }

    #[test]
    fn pls_keys_are_case_insensitive_and_whitespace_tolerant() {
        let text = "\u{feff}[PLAYLIST]\r\nfile1 = http://a.example.com/one \r\nFILE2=https://b.example.com/two\r\n";
        assert_eq!(
            entries(text),
            ["http://a.example.com/one", "https://b.example.com/two"]
        );
    }

    #[test]
    fn a_pls_without_its_header_is_still_a_pls() {
        assert_eq!(
            entries("File1=http://a.example.com/one\nTitle1=A\n"),
            ["http://a.example.com/one"]
        );
    }

    #[test]
    fn m3u_skips_comments_and_extinf() {
        let text = "#EXTM3U\n#EXTINF:-1,Some Station\nhttp://a.example.com/one\n\n# a comment\n  http://b.example.com/two  \n";
        assert_eq!(
            entries(text),
            ["http://a.example.com/one", "http://b.example.com/two"]
        );
    }

    #[test]
    fn a_bare_list_of_addresses_works() {
        assert_eq!(
            entries("http://a.example.com/one\r\nhttp://b.example.com/two"),
            ["http://a.example.com/one", "http://b.example.com/two"]
        );
    }

    #[test]
    fn relative_entries_resolve_against_the_playlist_address() {
        let text = "#EXTM3U\n/stream/high.mp3\nlow.mp3\n//cdn.example.com/x\n";
        assert_eq!(
            entries(text),
            [
                "http://radio.example.com/stream/high.mp3",
                "http://radio.example.com/listen/low.mp3",
                "http://cdn.example.com/x"
            ]
        );
    }

    #[test]
    fn only_http_and_https_entries_survive_and_repeats_collapse() {
        let text = "C:\\Music\\song.mp3\nfile:///Users/x/song.mp3\nrtmp://a.example.com/live\nmms://b.example.com/x\nhttp://a.example.com/one\nhttp://a.example.com/one\nhttps://a.example.com/one\n";
        assert_eq!(
            entries(text),
            ["http://a.example.com/one", "https://a.example.com/one"]
        );
    }

    #[test]
    fn empty_and_garbage_documents_have_no_entries() {
        assert!(entries("").is_empty());
        assert!(entries("#EXTM3U\n").is_empty());
        assert!(entries("<html><body>Not found</body></html>").is_empty());
        assert!(entries("[playlist]\nNumberOfEntries=0\n").is_empty());
    }

    #[test]
    fn hls_is_told_apart_from_a_plain_extended_m3u() {
        let live = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:100\n#EXTINF:6.0,\nseg100.ts\n";
        assert_eq!(parse(live, &base()), Playlist::Hls);
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=128000\nlow/index.m3u8\n";
        assert_eq!(parse(master, &base()), Playlist::Hls);
        assert_eq!(
            parse(
                "#EXTM3U\n#EXTINF:6,x\nhttp://a.example.com/one.mp3\n",
                &base()
            ),
            Playlist::Entries(vec![Url::parse("http://a.example.com/one.mp3").unwrap()])
        );
    }

    #[test]
    fn unlabelled_bodies_are_recognised_from_their_start() {
        for playlist in [
            &b"#EXTM3U\n"[..],
            b"[playlist]\nFile1=x",
            b"\xef\xbb\xbf#EXTM3U",
            b"  \n#EXT-X-VERSION:3",
            b"http://a.example.com/x\n",
        ] {
            assert!(
                looks_like_playlist(playlist),
                "{}",
                String::from_utf8_lossy(playlist)
            );
        }
        for not_playlist in [
            &b"ID3\x03\x00\x00"[..],
            b"\xff\xfb\x90\x00",
            b"OggS\x00\x02",
            b"<html>",
            b"",
            b"http://\x00\x01 binary",
        ] {
            assert!(!looks_like_playlist(not_playlist), "{not_playlist:?}");
        }
    }
}
