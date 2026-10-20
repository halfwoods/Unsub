use std::fs::File;
use std::io::Read;
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::id3::v2::{
    Frame, Id3v2Tag, SyncTextContentType, SynchronizedTextFrame, TimestampFormat,
};
use lofty::iff::aiff::AiffFile;
use lofty::mpeg::MpegFile;
use lofty::tag::items::Lang;
use lofty::tag::{ItemKey, Tag};
use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::library::Library;
use crate::tags::{self, repair_mojibake};

/// Lyrics come from files and sidecars of any origin, and no song needs more.
const MAX_LYRICS_BYTES: usize = 256 * 1024;
const MAX_SYNCED_LINES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    /// The words, line by line. When only timed lyrics exist, their text.
    pub text: String,
    /// Lines with their start in milliseconds, in time order.
    pub synced: Option<Vec<(u64, String)>>,
    pub source: LyricsSource,
}

/// Where `Lyrics::text` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LyricsSource {
    /// The audio file's own tags.
    Tag,
    /// A `.lrc` file beside the audio file, which supplied the timed lines.
    Sidecar,
    /// Set in Get Info for a file that could not be written, and kept in the
    /// library.
    Library,
}

/// What the audio file's tags hold.
#[derive(Default)]
struct Embedded {
    text: Option<String>,
    synced: Option<Vec<(u64, String)>>,
}

impl Library {
    /// A track's lyrics, read on demand: ID3 `USLT` (in the file's own
    /// language, else the first) and `SYLT`, MP4 `©lyr`, Vorbis `LYRICS` or
    /// `UNSYNCEDLYRICS`, APE `Lyrics`. A `.lrc` file with the same name beside
    /// the audio takes precedence for the timed lines. Lyrics set in Get Info
    /// for a file that could not be written win for the plain text. A file
    /// still only a placeholder in the cloud is not opened (that would
    /// download it), and neither is a placeholder sidecar.
    ///
    /// The text is untrusted: it is cut at 256 KB, stripped of control
    /// characters and given `\n` line endings.
    pub fn lyrics(&self, id: i64) -> Result<Option<Lyrics>> {
        let row: Option<(String, bool)> = self
            .conn
            .query_row(
                "SELECT path, in_cloud FROM tracks WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (path, in_cloud) = row.ok_or(Error::TrackNotFound(id))?;
        let path = Path::new(&path);
        let pinned = self.stored_overrides(id)?.and_then(|o| o.lyrics);
        let embedded = if in_cloud || (self.is_dataless)(path) {
            Embedded::default()
        } else {
            read_embedded(path)?
        };
        Ok(assemble(pinned, embedded, self.read_sidecar(path)))
    }

    /// An unreadable sidecar is the same as none: it is an extra.
    fn read_sidecar(&self, track: &Path) -> Option<Vec<(u64, String)>> {
        let sidecar = ["lrc", "LRC"]
            .into_iter()
            .map(|extension| track.with_extension(extension))
            .find(|candidate| candidate.is_file())
            .filter(|candidate| !(self.is_dataless)(candidate))?;
        let mut bytes = Vec::new();
        File::open(sidecar)
            .ok()?
            .take(MAX_LYRICS_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        parse_lrc(&String::from_utf8_lossy(&bytes))
    }
}

fn assemble(
    pinned: Option<String>,
    embedded: Embedded,
    sidecar: Option<Vec<(u64, String)>>,
) -> Option<Lyrics> {
    let (synced, synced_source) = match sidecar {
        Some(lines) => (Some(lines), LyricsSource::Sidecar),
        None => (embedded.synced, LyricsSource::Tag),
    };
    let (text, source) = match pinned {
        Some(text) => (clean(&text), LyricsSource::Library),
        None => (embedded.text.unwrap_or_default(), LyricsSource::Tag),
    };
    let (text, source) = match &synced {
        Some(lines) if text.is_empty() => {
            let words: Vec<&str> = lines.iter().map(|(_, line)| line.as_str()).collect();
            (words.join("\n").trim().to_owned(), synced_source)
        }
        _ => (text, source),
    };
    (!text.is_empty()).then_some(Lyrics {
        text,
        synced,
        source,
    })
}

fn read_embedded(path: &Path) -> Result<Embedded> {
    let options = ParseOptions::new().read_cover_art(false);
    let probe = tags::probe(path, options)?;
    Ok(match probe.file_type() {
        // ID3 is read as itself for the language choice and the timed frame.
        Some(FileType::Mpeg) => {
            let file = MpegFile::read_from(&mut probe.into_inner(), options)?;
            let mut found = file.id3v2().map(id3_lyrics).unwrap_or_default();
            if found.text.is_none() {
                found.text = file.ape().and_then(|ape| tag_text(&Tag::from(ape.clone())));
            }
            found
        }
        Some(FileType::Aiff) => {
            let file = AiffFile::read_from(&mut probe.into_inner(), options)?;
            file.id3v2().map(id3_lyrics).unwrap_or_default()
        }
        _ => Embedded {
            text: probe.read()?.tags().iter().find_map(tag_text),
            synced: None,
        },
    })
}

/// The lyrics a generic tag holds, as the file wrote them. `Lyrics` is
/// Vorbis' `LYRICS`, APE's `Lyrics` and MP4's `©lyr`; `UnsyncLyrics` is ID3's
/// `USLT` and Vorbis' `UNSYNCEDLYRICS`.
pub(crate) fn raw_text(tag: &Tag) -> Option<&str> {
    [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
        .into_iter()
        .filter_map(|key| tag.get_string(key))
        .find(|text| !text.trim().is_empty())
}

fn tag_text(tag: &Tag) -> Option<String> {
    raw_text(tag)
        .map(|text| clean(&repair_mojibake(text)))
        .filter(|text| !text.is_empty())
}

/// Several `USLT` and `SYLT` frames can sit in one tag, one per language.
fn id3_lyrics(tag: &Id3v2Tag) -> Embedded {
    let mut spoken: Option<Lang> = None;
    let mut texts: Vec<(Lang, String)> = Vec::new();
    let mut timed: Vec<(Lang, Vec<(u64, String)>)> = Vec::new();
    for frame in tag.clone() {
        if let Some(frame) = timed_frame(&frame) {
            let mut lines = Timed::default();
            for (ms, line) in frame.content {
                if !lines.push(u64::from(ms), clean(&repair_mojibake(&line))) {
                    break;
                }
            }
            if let Some(lines) = lines.finish(0) {
                timed.push((frame.language, lines));
            }
            continue;
        }
        match frame {
            Frame::Text(text) if text.id().as_str() == "TLAN" => {
                spoken = spoken.or_else(|| language_code(&text.value));
            }
            Frame::UnsynchronizedText(lyrics) => {
                let text = clean(&repair_mojibake(&lyrics.content));
                if !text.is_empty() {
                    texts.push((lyrics.language, text));
                }
            }
            _ => {}
        }
    }
    Embedded {
        text: pick(texts, spoken),
        synced: pick(timed, spoken),
    }
}

/// A `SYLT` frame holding lyrics timed in milliseconds. lofty leaves the rare
/// frame as bytes, and counting MPEG frames would need the stream's layout to
/// turn into time.
pub(crate) fn timed_frame(frame: &Frame<'_>) -> Option<SynchronizedTextFrame<'static>> {
    let Frame::Binary(binary) = frame else {
        return None;
    };
    if binary.id().as_str() != "SYLT" {
        return None;
    }
    SynchronizedTextFrame::parse(&binary.data, binary.flags())
        .ok()
        .filter(|frame| {
            frame.content_type == SyncTextContentType::Lyrics
                && frame.timestamp_format == TimestampFormat::MS
        })
}

/// `TLAN` holds ISO 639-2 codes, several of them separated by NULs.
fn language_code(value: &str) -> Option<Lang> {
    let code: Lang = value
        .split('\0')
        .next()?
        .trim()
        .to_ascii_lowercase()
        .as_bytes()
        .try_into()
        .ok()?;
    code.iter().all(u8::is_ascii_alphabetic).then_some(code)
}

/// The entry in the file's own language, or failing that the first.
fn pick<T>(entries: Vec<(Lang, T)>, language: Option<Lang>) -> Option<T> {
    let index = language
        .and_then(|wanted| {
            entries
                .iter()
                .position(|(lang, _)| lang.eq_ignore_ascii_case(&wanted))
        })
        .unwrap_or(0);
    entries.into_iter().nth(index).map(|(_, entry)| entry)
}

/// Makes text from a file, a sidecar or a caller fit to show: cut at the size
/// limit, `\n` line endings, no control characters (a tab becomes a space) and
/// no byte order mark, trimmed.
pub(crate) fn clean(text: &str) -> String {
    let text = &text[..text.floor_char_boundary(MAX_LYRICS_BYTES)];
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                chars.next_if_eq(&'\n');
                cleaned.push('\n');
            }
            '\n' => cleaned.push('\n'),
            '\t' => cleaned.push(' '),
            '\u{feff}' => {}
            c if c.is_control() => {}
            c => cleaned.push(c),
        }
    }
    cleaned.trim().to_owned()
}

/// Timed lines being collected, held to the same limits wherever they come from.
#[derive(Default)]
struct Timed {
    lines: Vec<(u64, String)>,
    bytes: usize,
}

impl Timed {
    /// False once the limits are reached and nothing more is taken.
    fn push(&mut self, ms: u64, line: String) -> bool {
        if self.lines.len() >= MAX_SYNCED_LINES || self.bytes + line.len() > MAX_LYRICS_BYTES {
            return false;
        }
        self.bytes += line.len();
        self.lines.push((ms, line));
        true
    }

    /// In time order, moved by an LRC `offset` (positive is earlier). Nothing
    /// at all unless some line has words in it.
    fn finish(mut self, offset_ms: i64) -> Option<Vec<(u64, String)>> {
        let shift = offset_ms.unsigned_abs();
        for (ms, _) in &mut self.lines {
            *ms = if offset_ms >= 0 {
                ms.saturating_sub(shift)
            } else {
                ms.saturating_add(shift)
            };
        }
        self.lines.sort_by_key(|(ms, _)| *ms);
        self.lines
            .iter()
            .any(|(_, line)| !line.is_empty())
            .then_some(self.lines)
    }
}

/// `[mm:ss.xx]Words` lines. A line may carry several time tags (a repeated
/// chorus), `[offset:±ms]` moves them all, other `[key:value]` tags (artist,
/// title...) are ignored, and anything else that is not a timed line is
/// skipped, so damage costs only the lines it touches.
fn parse_lrc(text: &str) -> Option<Vec<(u64, String)>> {
    let mut timed = Timed::default();
    let mut offset_ms = 0i64;
    for line in clean(text).lines() {
        let mut rest = line.trim();
        let mut stamps = Vec::new();
        while let Some((tag, after)) = rest
            .strip_prefix('[')
            .and_then(|inner| inner.split_once(']'))
        {
            if let Some(ms) = time_tag(tag) {
                stamps.push(ms);
            } else {
                if let Some((key, value)) = tag.split_once(':')
                    && key.trim().eq_ignore_ascii_case("offset")
                {
                    offset_ms = value.trim().parse().unwrap_or(offset_ms);
                }
                break;
            }
            rest = after.trim_start();
        }
        for ms in stamps {
            if !timed.push(ms, rest.to_owned()) {
                return timed.finish(offset_ms);
            }
        }
    }
    timed.finish(offset_ms)
}

/// "mm:ss" or "mm:ss.xx", to milliseconds.
fn time_tag(tag: &str) -> Option<u64> {
    let (minutes, seconds) = tag.trim().split_once(':')?;
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if minutes.is_empty() || whole.is_empty() || ![minutes, whole, fraction].into_iter().all(digits)
    {
        return None;
    }
    let minutes: u64 = minutes.parse().ok()?;
    let seconds: u64 = whole.parse().ok().filter(|s| *s < 60)?;
    let millis: u64 = format!("{fraction:0<3}")[..3].parse().ok()?;
    minutes
        .checked_mul(60)?
        .checked_add(seconds)?
        .checked_mul(1_000)?
        .checked_add(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<(u64, String)> {
        parse_lrc(text).unwrap_or_default()
    }

    fn timed(expected: &[(u64, &str)]) -> Vec<(u64, String)> {
        expected
            .iter()
            .map(|(ms, line)| (*ms, (*line).to_owned()))
            .collect()
    }

    #[test]
    fn time_tags_read_minutes_seconds_and_fractions() {
        for (tag, ms) in [
            ("00:00", 0),
            ("01:02", 62_000),
            ("01:02.5", 62_500),
            ("01:02.50", 62_500),
            ("01:02.05", 62_050),
            ("01:02.345", 62_345),
            ("01:02.3456", 62_345),
            ("75:20.00", 4_520_000),
            (" 00:10.00 ", 10_000),
        ] {
            assert_eq!(time_tag(tag), Some(ms), "{tag:?}");
        }
        for tag in [
            "",
            "00",
            "00:",
            ":10",
            "00:60",
            "00:10.x",
            "a:b",
            "00:-1",
            "+1:00",
            "00:10:00",
            "ar:Artist",
            "offset:+300",
        ] {
            assert_eq!(time_tag(tag), None, "{tag:?}");
        }
    }

    #[test]
    fn a_line_with_several_time_tags_repeats_and_lines_are_ordered() {
        let found = lines("[00:30.00][01:10.00]Chorus\n[00:10.00]Verse\n[00:20.50] \n");

        assert_eq!(
            found,
            timed(&[
                (10_000, "Verse"),
                (20_500, ""),
                (30_000, "Chorus"),
                (70_000, "Chorus")
            ])
        );
    }

    #[test]
    fn offset_moves_every_line_and_positive_is_earlier() {
        let earlier = lines("[offset:+500]\n[00:01.00]a\n[00:00.20]b");
        assert_eq!(earlier, timed(&[(0, "b"), (500, "a")]));

        let later = lines("[00:01.00]a\n[offset: -250 ]");
        assert_eq!(later, timed(&[(1_250, "a")]));

        let junk = lines("[offset:soon]\n[00:01.00]a");
        assert_eq!(junk, timed(&[(1_000, "a")]));
    }

    #[test]
    fn metadata_and_damaged_lines_are_skipped_without_losing_the_rest() {
        let found = lines(
            "\u{feff}[ar:Someone]\n[ti:Song [live]]\n[by:me]\nplain words\n\n[00:05.00]kept\n\
             [00:99.00]bad seconds\n[xx:yy]nothing\n[00:06.00 unclosed\n[00:07.00]also kept\n\
             [00:08.00][Chorus] with a label\n",
        );

        assert_eq!(
            found,
            timed(&[
                (5_000, "kept"),
                (7_000, "also kept"),
                (8_000, "[Chorus] with a label")
            ])
        );
    }

    #[test]
    fn nothing_usable_is_nothing() {
        for text in [
            "",
            "just words\non two lines",
            "[ar:A]\n[ti:T]",
            "[00:01.00]\n[00:02.00]  \n",
            "[00:xx]boom",
            "\0\0\0",
        ] {
            assert_eq!(parse_lrc(text), None, "{text:?}");
        }
    }

    #[test]
    fn timed_lines_are_bounded_in_count_and_size() {
        let many = "[00:01.00]x\n".repeat(MAX_SYNCED_LINES + 500);
        assert_eq!(parse_lrc(&many).unwrap().len(), MAX_SYNCED_LINES);

        // One long line repeated by thousands of time tags cannot multiply.
        let stamps = "[00:01.00]".repeat(5_000);
        let amplified = format!("{stamps}{}", "w".repeat(100_000));
        let found = parse_lrc(&amplified).unwrap();
        assert!(found.iter().map(|(_, line)| line.len()).sum::<usize>() <= MAX_LYRICS_BYTES);
        assert!(found.len() <= 2);
    }

    #[test]
    fn cleaning_normalises_line_endings_and_drops_control_characters() {
        assert_eq!(
            clean("\u{feff} one\r\ntwo\rthree\n\nfour\u{0}\u{7}\u{1b}[0m\u{85}\ttab "),
            "one\ntwo\nthree\n\nfour[0m tab"
        );
        assert_eq!(clean("  \n\t\u{0}\n"), "");
    }

    #[test]
    fn cleaning_cuts_at_the_limit_on_a_character_boundary() {
        // Two-byte characters put the limit mid-character on an odd boundary.
        let long = "\u{e9}".repeat(MAX_LYRICS_BYTES);

        let cleaned = clean(&long);

        assert_eq!(cleaned.len(), MAX_LYRICS_BYTES);
        assert!(cleaned.chars().all(|c| c == '\u{e9}'));
        let odd = format!("a{}", "\u{e9}".repeat(MAX_LYRICS_BYTES));
        assert_eq!(clean(&odd).len(), MAX_LYRICS_BYTES - 1);
    }

    #[test]
    fn the_file_s_own_language_wins_else_the_first() {
        let entries = || vec![(*b"fra", "fr"), (*b"eng", "en"), (*b"deu", "de")];

        assert_eq!(pick(entries(), Some(*b"eng")), Some("en"));
        assert_eq!(pick(entries(), Some(*b"ita")), Some("fr"));
        assert_eq!(pick(entries(), None), Some("fr"));
        assert_eq!(pick(Vec::<(Lang, &str)>::new(), Some(*b"eng")), None);
        assert_eq!(pick(vec![(*b"ENG", "x")], Some(*b"eng")), Some("x"));
    }

    #[test]
    fn declared_languages_are_codes() {
        assert_eq!(language_code("eng"), Some(*b"eng"));
        assert_eq!(language_code(" ENG\0fra"), Some(*b"eng"));
        for value in ["", "en", "english", "e1g", "\0eng"] {
            assert_eq!(language_code(value), None, "{value:?}");
        }
    }

    #[test]
    fn timed_lines_stand_in_for_missing_text_and_sidecar_beats_tag() {
        let tag_lines = vec![(1_000, "from tag".to_owned())];
        let sidecar_lines = vec![(2_000, "from sidecar".to_owned())];

        let only_timed = assemble(
            None,
            Embedded {
                text: None,
                synced: Some(tag_lines.clone()),
            },
            None,
        )
        .unwrap();
        assert_eq!(only_timed.text, "from tag");
        assert_eq!(only_timed.source, LyricsSource::Tag);

        let both = assemble(
            None,
            Embedded {
                text: Some("plain".into()),
                synced: Some(tag_lines),
            },
            Some(sidecar_lines),
        )
        .unwrap();
        assert_eq!(both.text, "plain");
        assert_eq!(both.source, LyricsSource::Tag);
        assert_eq!(both.synced.unwrap()[0].1, "from sidecar");

        assert_eq!(assemble(None, Embedded::default(), None), None);
        assert_eq!(
            assemble(Some("  ".into()), Embedded::default(), None),
            None,
            "lyrics cleared in the library stay cleared"
        );
    }
}
