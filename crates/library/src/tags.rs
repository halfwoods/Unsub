use std::borrow::Cow;
use std::ffi::OsStr;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::id3::v2::FrameId;
use lofty::mp4::{Mp4Codec, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag};

use crate::error::{Error, Result};
use crate::model::MediaKind;

/// Bump whenever `read` starts extracting something differently, so a rescan
/// re-reads files whose size and mtime say they are unchanged.
pub(crate) const TAG_PARSER_VERSION: u32 = 2;

/// Everything derived from the file's contents; filesystem facts (size, mtime)
/// are filled in by the scanner.
pub(crate) struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub composer: String,
    pub year: Option<i32>,
    pub track_number: Option<u32>,
    pub track_count: Option<u32>,
    pub disc_number: Option<u32>,
    pub disc_count: Option<u32>,
    pub duration_ms: u64,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub codec: &'static str,
    pub kind: MediaKind,
    pub compilation: bool,
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
}

struct Parsed {
    file: TaggedFile,
    codec: &'static str,
    id3_podcast_frame: bool,
}

pub(crate) fn read(path: &Path) -> Result<TrackMeta> {
    // Artwork is loaded on demand; skipping it keeps bulk scans fast.
    let options = ParseOptions::new().read_cover_art(false);
    let Parsed {
        file,
        codec,
        id3_podcast_frame,
    } = parse(path, options)?;
    let properties = file.properties();
    let tag = file.primary_tag().or_else(|| file.first_tag());

    let genre = text(tag, ItemKey::Genre);
    let (track_number, track_count) =
        number_and_total(tag, ItemKey::TrackNumber, ItemKey::TrackTotal);
    let (disc_number, disc_count) = number_and_total(tag, ItemKey::DiscNumber, ItemKey::DiscTotal);

    let mut title = text(tag, ItemKey::TrackTitle);
    if title.is_empty() {
        title = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
    }

    Ok(TrackMeta {
        title,
        artist: text(tag, ItemKey::TrackArtist),
        album_artist: text(tag, ItemKey::AlbumArtist),
        album: text(tag, ItemKey::AlbumTitle),
        composer: text(tag, ItemKey::Composer),
        year: tag
            .and_then(|t| t.date())
            .map(|date| i32::from(date.year))
            .filter(|year| *year > 0),
        track_number,
        track_count,
        disc_number,
        disc_count,
        duration_ms: properties.duration().as_millis() as u64,
        bitrate_kbps: properties
            .audio_bitrate()
            .or(properties.overall_bitrate())
            .filter(|kbps| *kbps > 0),
        sample_rate: properties.sample_rate().filter(|hz| *hz > 0),
        bit_depth: properties.bit_depth().filter(|bits| *bits > 0),
        channels: properties.channels().filter(|count| *count > 0),
        codec,
        kind: media_kind(path, tag, &genre, id3_podcast_frame),
        compilation: flag(tag, ItemKey::FlagCompilation),
        sort_title: optional_text(tag, ItemKey::TrackTitleSortOrder),
        sort_artist: optional_text(tag, ItemKey::TrackArtistSortOrder),
        sort_album: optional_text(tag, ItemKey::AlbumTitleSortOrder),
        sort_album_artist: optional_text(tag, ItemKey::AlbumArtistSortOrder),
        genre,
    })
}

/// Content sniffing wins over the extension, so mislabelled files still scan.
pub(crate) fn probe(path: &Path, options: ParseOptions) -> Result<Probe<BufReader<File>>> {
    Ok(Probe::open(path)?.options(options).guess_file_type()?)
}

fn parse(path: &Path, options: ParseOptions) -> Result<Parsed> {
    let probe = probe(path, options)?;
    let codec = match probe.file_type() {
        Some(FileType::Mp4) => return parse_mp4(probe, options),
        Some(FileType::Mpeg) => return parse_mpeg(probe, options),
        Some(FileType::Flac) => "FLAC",
        Some(FileType::Aac) => "AAC",
        Some(FileType::Vorbis) => "Vorbis",
        Some(FileType::Opus) => "Opus",
        Some(FileType::Wav) => "WAV",
        Some(FileType::Aiff) => "AIFF",
        Some(FileType::WavPack) => "WavPack",
        Some(FileType::Ape) => "APE",
        _ => return Err(Error::UnsupportedFormat),
    };
    Ok(Parsed {
        file: probe.read()?,
        codec,
        id3_podcast_frame: false,
    })
}

/// lofty's generic `TaggedFile` loses the MP4 codec, which is the only way
/// to tell ALAC from AAC, so MP4 is read through its concrete type.
fn parse_mp4(probe: Probe<BufReader<File>>, options: ParseOptions) -> Result<Parsed> {
    let mp4 = Mp4File::read_from(&mut probe.into_inner(), options)?;
    let codec = match mp4.properties().codec() {
        Some(Mp4Codec::ALAC) => "ALAC",
        Some(Mp4Codec::MP3) => "MP3",
        Some(Mp4Codec::FLAC) => "FLAC",
        _ => "AAC",
    };
    Ok(Parsed {
        file: mp4.into(),
        codec,
        id3_podcast_frame: false,
    })
}

/// iTunes writes the ID3 `PCST` flag as a binary frame, which the generic
/// `Tag` drops, so it is only visible on the concrete type. (The MP4 `pcst`
/// flag does survive as `ItemKey::FlagPodcast`.)
fn parse_mpeg(probe: Probe<BufReader<File>>, options: ParseOptions) -> Result<Parsed> {
    let mpeg = MpegFile::read_from(&mut probe.into_inner(), options)?;
    let id3_podcast_frame = mpeg
        .id3v2()
        .is_some_and(|tag| tag.get(&FrameId::Valid(Cow::Borrowed("PCST"))).is_some());
    Ok(Parsed {
        file: mpeg.into(),
        codec: "MP3",
        id3_podcast_frame,
    })
}

fn media_kind(path: &Path, tag: Option<&Tag>, genre: &str, id3_podcast_frame: bool) -> MediaKind {
    let is_m4b = path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("m4b"));
    if is_m4b || matches_genre(genre, &["audiobook", "audiobooks"]) {
        MediaKind::Audiobook
    } else if id3_podcast_frame
        || flag(tag, ItemKey::FlagPodcast)
        || matches_genre(genre, &["podcast", "podcasts"])
    {
        MediaKind::Podcast
    } else {
        MediaKind::Music
    }
}

fn matches_genre(genre: &str, names: &[&str]) -> bool {
    names.iter().any(|name| genre.eq_ignore_ascii_case(name))
}

fn optional_text(tag: Option<&Tag>, key: ItemKey) -> Option<String> {
    Some(text(tag, key)).filter(|value| !value.is_empty())
}

fn text(tag: Option<&Tag>, key: ItemKey) -> String {
    tag.and_then(|t| t.get_string(key))
        .map(|value| repair_mojibake(value).trim().to_owned())
        .unwrap_or_default()
}

/// Many ID3 frames are declared ISO-8859-1 but hold UTF-8 bytes, which lofty
/// then reads as "JÃ³zsef". Re-decoding the Latin-1 bytes as UTF-8 undoes it,
/// but only when the whole string decodes cleanly: genuine Latin-1 such as
/// "SÃO" is not valid UTF-8 and stays as it is.
fn repair_mojibake(text: &str) -> String {
    if text.is_ascii() {
        return text.to_owned();
    }
    let latin1_bytes: Option<Vec<u8>> = text.chars().map(|c| u8::try_from(c).ok()).collect();
    latin1_bytes
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| text.to_owned())
}

fn flag(tag: Option<&Tag>, key: ItemKey) -> bool {
    tag.and_then(|t| t.get_string(key)) == Some("1")
}

/// Vorbis and some taggers store "3/12" in the number field rather than
/// splitting it, so the total can come from either key.
fn number_and_total(
    tag: Option<&Tag>,
    number_key: ItemKey,
    total_key: ItemKey,
) -> (Option<u32>, Option<u32>) {
    let part = |key: ItemKey, index: usize| {
        tag.and_then(|t| t.get_string(key))
            .and_then(|value| value.split('/').nth(index))
            .and_then(|part| part.trim().parse::<u32>().ok())
    };
    (
        part(number_key, 0),
        part(total_key, 0).or_else(|| part(number_key, 1)),
    )
}

#[cfg(test)]
mod tests {
    use super::repair_mojibake;

    #[test]
    fn utf8_read_as_latin1_is_repaired() {
        let repaired = [
            ("Garai J\u{c3}\u{b3}zsef", "Garai J\u{f3}zsef"),
            ("\u{c2}\u{a9} Klankbeeld", "\u{a9} Klankbeeld"),
            ("Beyonc\u{c3}\u{a9}", "Beyonc\u{e9}"),
            (
                "Mot\u{c3}\u{b6}rhead \u{c3}\u{bc}ber \u{c3}\u{a4}lles",
                "Mot\u{f6}rhead \u{fc}ber \u{e4}lles",
            ),
            // Three- and four-byte sequences land on C1 control characters.
            ("Don\u{e2}\u{80}\u{99}t Stop", "Don\u{2019}t Stop"),
            ("\u{f0}\u{9f}\u{8e}\u{b5} Music", "\u{1f3b5} Music"),
            ("\u{c2}\u{a0}", "\u{a0}"),
        ];
        for (garbled, expected) in repaired {
            assert_eq!(repair_mojibake(garbled), expected, "{garbled:?}");
        }
    }

    #[test]
    fn text_that_is_not_clean_utf8_in_disguise_is_left_alone() {
        let untouched = [
            "",
            "Plain ASCII",
            // Correct text, including lone Latin-1 bytes that are invalid UTF-8.
            "Garai J\u{f3}zsef",
            "Beyonc\u{e9}",
            "S\u{c3}O PAULO",
            "NO\u{cb}L",
            "\u{c3}",
            "\u{c3}(",
            "\u{c2}\u{a9} then \u{e9}",
            // Anything outside Latin-1 cannot be a byte string.
            "\u{65e5}\u{672c}\u{8a9e}",
            "Garai J\u{c3}\u{b3}zsef \u{2013} \u{65e5}\u{672c}",
        ];
        for text in untouched {
            assert_eq!(repair_mojibake(text), text, "{text:?}");
        }
    }

    #[test]
    fn repairing_twice_changes_nothing_more() {
        let once = repair_mojibake("Garai J\u{c3}\u{b3}zsef");

        assert_eq!(repair_mojibake(&once), once);
    }
}
