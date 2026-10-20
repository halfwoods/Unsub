//! SHOUTcast/Icecast metadata.
//!
//! A client that sends `Icy-MetaData: 1` gets an `icy-metaint: N` header back, and from then on the
//! body is N bytes of audio, one length byte, `length * 16` bytes of metadata (usually zero), N bytes
//! of audio, and so on. The metadata is text like `StreamTitle='Artist - Title';StreamUrl='';`
//! in whatever encoding the broadcaster's software used.

use std::{
    io::{self, Read},
    sync::mpsc::Sender,
};

use serde::{Deserialize, Serialize};

use super::Event;
use crate::text::decode;

/// What is playing now, as the station says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NowPlaying {
    /// The whole `StreamTitle`. Empty when the station has cleared it (between songs, say).
    pub raw: String,
    /// Set when the title reads "Artist - Title".
    pub artist: Option<String>,
    pub title: Option<String>,
}

impl NowPlaying {
    pub(crate) fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        // Stations write " - " between artist and title. Only the first splits: the title may
        // contain more ("Song - Radio Edit").
        let split = raw
            .split_once(" - ")
            .filter(|(artist, title)| !artist.trim().is_empty() && !title.trim().is_empty());
        match split {
            Some((artist, title)) => Self {
                raw: raw.to_owned(),
                artist: Some(artist.trim().to_owned()),
                title: Some(title.trim().to_owned()),
            },
            None => Self {
                raw: raw.to_owned(),
                artist: None,
                title: (!raw.is_empty()).then(|| raw.to_owned()),
            },
        }
    }
}

/// The fields of a metadata block that matter here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Block {
    pub stream_title: Option<String>,
}

/// Reads the `Key='value';` pairs of a metadata block. A value may itself contain a quote or a
/// semicolon (`StreamTitle='Don't Stop';`), so a value ends at the quote-semicolon that is followed
/// by another key or by the end of the block.
pub(crate) fn parse_block(bytes: &[u8]) -> Block {
    let text = decode(bytes);
    let mut block = Block::default();
    let mut rest = text.as_str();
    while let Some((key, after_key)) = rest.split_once("='") {
        let key = key.trim_start_matches(';').trim();
        let mut end = None;
        let mut search = 0;
        while let Some(found) = after_key[search..].find("';") {
            let at = search + found;
            let after = &after_key[at + 2..];
            if after.trim().is_empty() || starts_with_key(after) {
                end = Some(at);
                break;
            }
            search = at + 2;
        }
        let (value, next) = match end {
            Some(at) => (&after_key[..at], &after_key[at + 2..]),
            // The last value, perhaps without its closing `';`.
            None => (
                after_key
                    .trim_end_matches([';', '\''])
                    .trim_end_matches('\0'),
                "",
            ),
        };
        if key.eq_ignore_ascii_case("StreamTitle") {
            block.stream_title = Some(value.trim().to_owned());
        }
        rest = next;
    }
    block
}

/// Whether `text` begins with `Word='`, the start of the next pair.
fn starts_with_key(text: &str) -> bool {
    let trimmed = text.trim_start();
    let key_len = trimmed
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    key_len > 0 && trimmed[key_len..].starts_with("='")
}

/// Removes the metadata from a body, passing the audio through and reporting titles as they change.
pub(crate) struct Demuxer<R> {
    inner: R,
    metaint: usize,
    /// Audio bytes before the next length byte.
    until_meta: usize,
    /// Audio handed out on this connection.
    delivered: u64,
    /// Audio handed out on earlier connections, so offsets count from the start of the listening.
    base: u64,
    last_title: Option<String>,
    events: Sender<Event>,
}

impl<R: Read> Demuxer<R> {
    pub fn new(inner: R, metaint: usize, base: u64, events: Sender<Event>) -> Self {
        Self {
            inner,
            metaint,
            until_meta: metaint,
            delivered: 0,
            base,
            last_title: None,
            events,
        }
    }

    /// Audio bytes produced since the listening began, across connections.
    pub fn position(&self) -> u64 {
        self.base + self.delivered
    }

    fn read_metadata(&mut self) -> io::Result<()> {
        let mut length = [0u8; 1];
        self.inner.read_exact(&mut length)?;
        self.until_meta = self.metaint;
        if length[0] == 0 {
            return Ok(());
        }
        let mut block = vec![0u8; usize::from(length[0]) * 16];
        self.inner.read_exact(&mut block)?;
        if let Some(title) = parse_block(&block).stream_title {
            // A station repeats the same block after every metaint of audio; only changes count.
            if self.last_title.as_deref() != Some(title.as_str())
                && !(self.last_title.is_none() && title.is_empty())
            {
                let track = NowPlaying::parse(&title);
                self.last_title = Some(title);
                // Nobody listening for events is not a reason to stop the audio.
                let _ = self.events.send(Event::NowPlaying {
                    track,
                    offset: self.position(),
                });
            }
        }
        Ok(())
    }
}

impl<R: Read> Read for Demuxer<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.metaint == 0 {
            let read = self.inner.read(buf)?;
            self.delivered += read as u64;
            return Ok(read);
        }
        if self.until_meta == 0 {
            self.read_metadata()?;
        }
        let wanted = buf.len().min(self.until_meta);
        let read = self.inner.read(&mut buf[..wanted])?;
        self.until_meta -= read;
        self.delivered += read as u64;
        Ok(read)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::io::Read;

    /// Hands out at most `step` bytes per read, cycling through the given sizes, so every boundary
    /// in a body gets split at some point.
    pub struct Trickle<R> {
        pub inner: R,
        pub sizes: Vec<usize>,
        pub next: usize,
    }

    impl<R: Read> Read for Trickle<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let size = self.sizes[self.next % self.sizes.len()];
            self.next += 1;
            let wanted = buf.len().min(size.max(1));
            self.inner.read(&mut buf[..wanted])
        }
    }

    /// A body of `audio` split into `metaint`-byte runs with `blocks` (padded to 16 bytes) between.
    pub fn interleave(audio: &[u8], metaint: usize, blocks: &[&[u8]]) -> Vec<u8> {
        let mut body = Vec::new();
        for (index, run) in audio.chunks(metaint).enumerate() {
            body.extend(run);
            if run.len() == metaint {
                let block = blocks.get(index).copied().unwrap_or(&[]);
                body.extend(frame_block(block));
            }
        }
        body
    }

    /// A metadata block as it goes on the wire: its length in 16-byte units, then the text padded
    /// with NULs.
    pub fn frame_block(text: &[u8]) -> Vec<u8> {
        if text.is_empty() {
            return vec![0];
        }
        let units = text.len().div_ceil(16);
        let mut framed = vec![u8::try_from(units).unwrap()];
        framed.extend(text);
        framed.resize(1 + units * 16, 0);
        framed
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, sync::mpsc};

    use super::{testing::*, *};

    fn demux(body: Vec<u8>, metaint: usize, sizes: Vec<usize>) -> (Vec<u8>, Vec<Event>) {
        let (tx, rx) = mpsc::channel();
        let mut demuxer = Demuxer::new(
            Trickle {
                inner: Cursor::new(body),
                sizes,
                next: 0,
            },
            metaint,
            0,
            tx,
        );
        let mut audio = Vec::new();
        let mut buf = [0u8; 100];
        loop {
            match demuxer.read(&mut buf) {
                Ok(0) => break,
                Ok(read) => audio.extend(&buf[..read]),
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(error) => panic!("{error}"),
            }
        }
        (audio, rx.try_iter().collect())
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 253) as u8).collect()
    }

    fn titles(events: &[Event]) -> Vec<(String, u64)> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::NowPlaying { track, offset } => Some((track.raw.clone(), *offset)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn audio_comes_through_whole_whatever_the_metaint_and_the_read_sizes() {
        let audio = pattern(5000);
        for metaint in [1, 7, 16, 100, 4096, 5000, 9000] {
            for sizes in [
                vec![1],
                vec![3, 1, 7],
                vec![16, 17],
                vec![4096],
                vec![2, 100, 5, 13],
            ] {
                let blocks: Vec<&[u8]> = vec![b"StreamTitle='One';", b"", b"StreamTitle='Two';"];
                let body = interleave(&audio, metaint, &blocks);
                let (got, _) = demux(body, metaint, sizes.clone());
                assert_eq!(got, audio, "metaint {metaint}, sizes {sizes:?}");
            }
        }
    }

    #[test]
    fn titles_are_reported_with_the_audio_offset_where_they_start() {
        let audio = pattern(1000);
        let blocks: Vec<&[u8]> = vec![
            b"StreamTitle='First - Song';StreamUrl='';",
            b"",
            b"StreamTitle='Second - Song';",
        ];
        let (_, events) = demux(interleave(&audio, 250, &blocks), 250, vec![11]);
        assert_eq!(
            titles(&events),
            [
                ("First - Song".to_owned(), 250),
                ("Second - Song".to_owned(), 750)
            ]
        );
    }

    #[test]
    fn metadata_split_across_reads_is_reassembled() {
        let audio = pattern(300);
        let long = b"StreamTitle='A Rather Long Artist Name - An Equally Long Song Title (Live)';StreamUrl='http://example.com/';";
        let blocks: Vec<&[u8]> = vec![long];
        for step in 1..=20 {
            let (got, events) = demux(interleave(&audio, 100, &blocks), 100, vec![step]);
            assert_eq!(got, audio, "step {step}");
            assert_eq!(titles(&events).len(), 1, "step {step}");
            assert_eq!(
                titles(&events)[0].0,
                "A Rather Long Artist Name - An Equally Long Song Title (Live)"
            );
        }
    }

    #[test]
    fn a_repeated_title_is_reported_once() {
        let blocks: Vec<&[u8]> = vec![
            b"StreamTitle='Same';",
            b"StreamTitle='Same';",
            b"StreamTitle='Same';",
            b"StreamTitle='Other';",
        ];
        let (_, events) = demux(interleave(&pattern(500), 100, &blocks), 100, vec![50]);
        assert_eq!(
            titles(&events)
                .iter()
                .map(|(title, _)| title.as_str())
                .collect::<Vec<_>>(),
            ["Same", "Other"]
        );
    }

    #[test]
    fn an_empty_title_is_ignored_at_first_and_clears_a_known_one() {
        let blocks: Vec<&[u8]> = vec![
            b"StreamTitle='';",
            b"StreamTitle='Song';",
            b"StreamTitle='';",
            b"StreamTitle='';",
        ];
        let (_, events) = demux(interleave(&pattern(500), 100, &blocks), 100, vec![9]);
        let seen = titles(&events);
        assert_eq!(
            seen.iter()
                .map(|(title, _)| title.as_str())
                .collect::<Vec<_>>(),
            ["Song", ""]
        );
        let Event::NowPlaying { track, .. } = &events[1] else {
            panic!()
        };
        assert_eq!(
            (track.artist.as_deref(), track.title.as_deref()),
            (None, None)
        );
    }

    #[test]
    fn without_a_metaint_the_body_is_all_audio() {
        let audio = pattern(2000);
        let (got, events) = demux(audio.clone(), 0, vec![64]);
        assert_eq!(got, audio);
        assert!(events.is_empty());
    }

    #[test]
    fn a_body_cut_off_inside_metadata_is_an_error_not_garbage_audio() {
        let mut body = interleave(&pattern(100), 100, &[b"StreamTitle='Cut off here';"]);
        body.truncate(100 + 1 + 5);
        let (tx, _rx) = mpsc::channel();
        let mut demuxer = Demuxer::new(Cursor::new(body), 100, 0, tx);
        let mut buf = [0u8; 200];
        assert_eq!(demuxer.read(&mut buf).unwrap(), 100);
        assert_eq!(
            demuxer.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn offsets_continue_across_connections() {
        let (tx, rx) = mpsc::channel();
        let body = interleave(&pattern(200), 100, &[b"StreamTitle='Later';"]);
        let mut demuxer = Demuxer::new(Cursor::new(body), 100, 5_000, tx);
        let mut sink = Vec::new();
        let _ = demuxer.read_to_end(&mut sink);
        let events: Vec<Event> = rx.try_iter().collect();
        assert_eq!(titles(&events), [("Later".to_owned(), 5_100)]);
    }

    #[test]
    fn blocks_with_quotes_and_semicolons_inside_a_title() {
        let cases: [(&[u8], &str); 4] = [
            (b"StreamTitle='Don't Stop Believin';", "Don't Stop Believin"),
            (
                b"StreamTitle='Rock; Roll - Part 2';StreamUrl='http://x';",
                "Rock; Roll - Part 2",
            ),
            (
                b"StreamTitle='It's; a 'quote'; test';StreamUrl='';",
                "It's; a 'quote'; test",
            ),
            (b"StreamTitle='No closing quote", "No closing quote"),
        ];
        for (block, title) in cases {
            assert_eq!(
                parse_block(block).stream_title.as_deref(),
                Some(title),
                "{}",
                String::from_utf8_lossy(block)
            );
        }
    }

    #[test]
    fn blocks_without_a_title_and_with_other_fields_first() {
        assert_eq!(parse_block(b"StreamUrl='http://x';").stream_title, None);
        assert_eq!(parse_block(b"").stream_title, None);
        assert_eq!(parse_block(b"\0\0\0\0").stream_title, None);
        assert_eq!(
            parse_block(b"StreamUrl='http://x';StreamTitle='T';")
                .stream_title
                .as_deref(),
            Some("T")
        );
        assert_eq!(
            parse_block(b"streamtitle='Lower';").stream_title.as_deref(),
            Some("Lower")
        );
    }

    #[test]
    fn titles_are_utf8_when_they_can_be_and_windows_1252_when_they_cannot() {
        let utf8 = "StreamTitle='Bj\u{f6}rk - Hunter \u{2013} \u{1F3B5}';".as_bytes();
        assert_eq!(
            parse_block(utf8).stream_title.as_deref(),
            Some("Bj\u{f6}rk - Hunter \u{2013} \u{1F3B5}")
        );
        // Latin-1 bytes for "Bjork" with an o-umlaut, and Windows-1252 punctuation: 0x96 is an en dash.
        let latin1 = b"StreamTitle='Bj\xf6rk \x96 Caf\xe9 \x92n Roll';";
        assert_eq!(
            parse_block(latin1).stream_title.as_deref(),
            Some("Bj\u{f6}rk \u{2013} Caf\u{e9} \u{2019}n Roll")
        );
    }

    #[test]
    fn now_playing_splits_artist_from_title_only_where_the_station_does() {
        let cases: [(&str, Option<&str>, Option<&str>); 7] = [
            (
                "Queen - Bohemian Rhapsody",
                Some("Queen"),
                Some("Bohemian Rhapsody"),
            ),
            (
                "  Queen  -  Bohemian Rhapsody ",
                Some("Queen"),
                Some("Bohemian Rhapsody"),
            ),
            (
                "Artist - Song - Radio Edit",
                Some("Artist"),
                Some("Song - Radio Edit"),
            ),
            ("AC-DC", None, Some("AC-DC")),
            ("Just a Title", None, Some("Just a Title")),
            (" - Orphan", None, Some("- Orphan")),
            ("", None, None),
        ];
        for (raw, artist, title) in cases {
            let parsed = NowPlaying::parse(raw);
            assert_eq!(
                (parsed.artist.as_deref(), parsed.title.as_deref()),
                (artist, title),
                "{raw:?}"
            );
            assert_eq!(parsed.raw, raw.trim());
        }
    }
}
