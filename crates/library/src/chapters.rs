use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::{Frame, Id3v2Tag};
use lofty::iff::aiff::AiffFile;
use lofty::mpeg::MpegFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OpusFile, VorbisFile};
use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::MediaKind;
use crate::tags;

/// A file can declare as many chapters as it likes; none has a use for this many.
const MAX_CHAPTERS: usize = 10_000;
const MAX_TITLE_CHARS: usize = 500;
/// The movie box of a very long audiobook holds a table entry per audio frame.
const MAX_MOOV_BYTES: u64 = 128 * 1024 * 1024;
/// A chapter title is a few bytes, so a text sample bigger than this is not one.
const MAX_TEXT_SAMPLE_BYTES: u32 = 64 * 1024;
const MAX_TOC_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub title: String,
    pub start_ms: u64,
    /// Where the chapter ends. Formats that only mark starts get the next
    /// chapter's start, or the end of the audio for the last; it is absent only
    /// when that length is unknown.
    pub end_ms: Option<u64>,
}

/// A chapter as a container states it, before it is checked.
struct Raw {
    title: String,
    start_ms: u64,
    end_ms: Option<u64>,
}

/// What a container yielded, and how long it says the audio is.
struct Found {
    chapters: Vec<Raw>,
    duration_ms: Option<u64>,
}

impl Library {
    /// The chapters in a track's file, read on demand: QuickTime chapter tracks
    /// and Nero `chpl` in MP4/M4B, `CHAP`/`CTOC` in ID3v2 (MP3, AIFF), and
    /// `CHAPTERnnn` comments in Ogg Vorbis, Opus and FLAC. A file without any, in
    /// another format, a station, or still only a placeholder in the cloud
    /// (reading it would download it) has none. Damaged chapter data yields whatever
    /// chapters survive rather than an error.
    pub fn chapters(&self, id: i64) -> Result<Vec<Chapter>> {
        let row: Option<(String, bool, u64, MediaKind)> = self
            .conn
            .query_row(
                "SELECT path, in_cloud, duration_ms, kind FROM tracks WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let (path, in_cloud, duration_ms, kind) = row.ok_or(Error::TrackNotFound(id))?;
        let path = Path::new(&path);
        if in_cloud || kind == MediaKind::Radio || (self.is_dataless)(path) {
            return Ok(Vec::new());
        }
        read_chapters(path, duration_ms)
    }
}

pub(crate) fn read_chapters(path: &Path, library_duration_ms: u64) -> Result<Vec<Chapter>> {
    let options = ParseOptions::new().read_cover_art(false);
    let probe = tags::probe(path, options)?;
    let found = match probe.file_type() {
        Some(FileType::Mp4) => mp4_chapters(path)?,
        Some(FileType::Mpeg) => {
            let file = MpegFile::read_from(&mut probe.into_inner(), options)?;
            Found {
                chapters: file.id3v2().map(id3_chapters).unwrap_or_default(),
                duration_ms: millis(file.properties().duration()),
            }
        }
        Some(FileType::Aiff) => {
            let file = AiffFile::read_from(&mut probe.into_inner(), options)?;
            Found {
                chapters: file.id3v2().map(id3_chapters).unwrap_or_default(),
                duration_ms: millis(file.properties().duration()),
            }
        }
        Some(FileType::Vorbis) => {
            let file = VorbisFile::read_from(&mut probe.into_inner(), options)?;
            Found {
                chapters: vorbis_chapters(file.vorbis_comments()),
                duration_ms: millis(file.properties().duration()),
            }
        }
        Some(FileType::Opus) => {
            let file = OpusFile::read_from(&mut probe.into_inner(), options)?;
            Found {
                chapters: vorbis_chapters(file.vorbis_comments()),
                duration_ms: millis(file.properties().duration()),
            }
        }
        Some(FileType::Flac) => {
            let file = FlacFile::read_from(&mut probe.into_inner(), options)?;
            Found {
                chapters: file
                    .vorbis_comments()
                    .map(vorbis_chapters)
                    .unwrap_or_default(),
                duration_ms: millis(file.properties().duration()),
            }
        }
        _ => return Ok(Vec::new()),
    };
    let duration_ms = found.duration_ms.unwrap_or(library_duration_ms);
    Ok(finish(found.chapters, duration_ms))
}

fn millis(duration: std::time::Duration) -> Option<u64> {
    Some(duration.as_millis() as u64).filter(|ms| *ms > 0)
}

/// Applies what holds for every container. Chapters that start at or after the
/// end of the audio are dropped and ends are held to it, which also catches a
/// time field read in the wrong unit. Titles are trimmed, stripped of control
/// characters and cut short; a chapter without one is named by its place.
/// Missing ends come from the next chapter. A zero `duration_ms` means unknown.
fn finish(raw: Vec<Raw>, duration_ms: u64) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = raw
        .into_iter()
        .filter(|chapter| duration_ms == 0 || chapter.start_ms < duration_ms)
        .take(MAX_CHAPTERS)
        .enumerate()
        .map(|(index, chapter)| {
            let title: String = chapter
                .title
                .chars()
                .filter(|c| !c.is_control())
                .take(MAX_TITLE_CHARS)
                .collect();
            let title = title.trim();
            Chapter {
                title: if title.is_empty() {
                    format!("Chapter {}", index + 1)
                } else {
                    title.to_owned()
                },
                start_ms: chapter.start_ms,
                end_ms: chapter.end_ms.map(|end| {
                    if duration_ms == 0 {
                        end
                    } else {
                        end.min(duration_ms)
                    }
                }),
            }
        })
        .collect();

    let starts: Vec<u64> = chapters.iter().map(|chapter| chapter.start_ms).collect();
    for (index, chapter) in chapters.iter_mut().enumerate() {
        let next = starts.get(index + 1).copied();
        let fallback = next.or(Some(duration_ms).filter(|ms| *ms > 0));
        chapter.end_ms = chapter
            .end_ms
            .filter(|end| *end > chapter.start_ms)
            .or(fallback.filter(|end| *end > chapter.start_ms));
    }
    chapters
}

// ---------------------------------------------------------------- ID3v2

struct Toc {
    id: String,
    top_level: bool,
    ordered: bool,
    entries: Vec<String>,
}

/// `CHAP` frames are the chapters (their `TIT2` sub-frame the title); `CTOC`
/// frames say in what order. An ordered top-level table of contents is followed
/// as written, nested tables included. Without one, or when it is unordered,
/// chapters go by start time, and any the table leaves out follow the rest.
fn id3_chapters(tag: &Id3v2Tag) -> Vec<Raw> {
    let mut chapters: Vec<(String, Raw)> = Vec::new();
    let mut tocs: Vec<Toc> = Vec::new();
    for frame in tag.clone() {
        match frame {
            Frame::Chapter(chapter) => {
                let title = chapter
                    .children
                    .into_iter()
                    .find_map(|child| match child {
                        Frame::Text(text) if text.id().as_str() == "TIT2" => {
                            Some(text.value.into_owned())
                        }
                        _ => None,
                    })
                    .unwrap_or_default();
                let (start, end) = (u64::from(chapter.times.start), u64::from(chapter.times.end));
                chapters.push((
                    chapter.id.into_owned(),
                    Raw {
                        title,
                        start_ms: start,
                        // An end at or before the start, or the maximum, is no end.
                        end_ms: Some(end).filter(|end| *end > start && *end != u64::from(u32::MAX)),
                    },
                ));
            }
            Frame::TableOfContents(toc) => tocs.push(Toc {
                id: toc.id.into_owned(),
                top_level: toc.flags.top_level,
                ordered: toc.flags.ordered,
                entries: toc.entries.iter().map(ToString::to_string).collect(),
            }),
            _ => {}
        }
    }

    let index_of: HashMap<&str, usize> = chapters
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (id.as_str(), index))
        .collect();
    let by_id: HashMap<&str, &Toc> = tocs.iter().map(|toc| (toc.id.as_str(), toc)).collect();

    let mut order: Vec<usize> = Vec::new();
    if let Some(top) = tocs.iter().find(|toc| toc.top_level).or(tocs.first()) {
        walk_toc(top, 0, &by_id, &index_of, &mut HashSet::new(), &mut order);
        let mut seen = HashSet::new();
        order.retain(|index| seen.insert(*index));
        if !top.ordered {
            order.sort_by_key(|index| chapters[*index].1.start_ms);
        }
    }
    let listed: HashSet<usize> = order.iter().copied().collect();
    let mut unlisted: Vec<usize> = (0..chapters.len())
        .filter(|i| !listed.contains(i))
        .collect();
    unlisted.sort_by_key(|index| chapters[*index].1.start_ms);
    order.extend(unlisted);

    let mut slots: Vec<Option<Raw>> = chapters.into_iter().map(|(_, raw)| Some(raw)).collect();
    order
        .into_iter()
        .filter_map(|index| slots[index].take())
        .collect()
}

/// Depth-first in the table's order. A table that names one it is already
/// inside, or nests too deep, is not followed further.
fn walk_toc<'a>(
    toc: &'a Toc,
    depth: usize,
    by_id: &HashMap<&str, &'a Toc>,
    index_of: &HashMap<&str, usize>,
    visited: &mut HashSet<&'a str>,
    order: &mut Vec<usize>,
) {
    if depth > MAX_TOC_DEPTH || !visited.insert(toc.id.as_str()) {
        return;
    }
    for entry in &toc.entries {
        if let Some(index) = index_of.get(entry.as_str()) {
            order.push(*index);
        } else if let Some(nested) = by_id.get(entry.as_str()) {
            walk_toc(nested, depth + 1, by_id, index_of, visited, order);
        }
    }
}

// -------------------------------------------------------- Vorbis comments

/// `CHAPTER001=00:00:00.000` with the title in `CHAPTER001NAME`.
fn vorbis_chapters(comments: &VorbisComments) -> Vec<Raw> {
    let mut starts: Vec<(u32, u64)> = Vec::new();
    let mut names: HashMap<u32, String> = HashMap::new();
    for (key, value) in comments.items() {
        let key = key.to_ascii_uppercase();
        let Some(rest) = key.strip_prefix("CHAPTER") else {
            continue;
        };
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let Ok(number) = rest[..digits].parse::<u32>() else {
            continue;
        };
        match &rest[digits..] {
            "" => {
                if let Some(ms) = parse_timestamp(value) {
                    starts.push((number, ms));
                }
            }
            "NAME" => {
                names.entry(number).or_insert_with(|| value.to_owned());
            }
            _ => {}
        }
    }
    starts.sort_by_key(|(number, ms)| (*ms, *number));
    starts
        .into_iter()
        .map(|(number, start_ms)| Raw {
            title: names.remove(&number).unwrap_or_default(),
            start_ms,
            end_ms: None,
        })
        .collect()
}

/// "HH:MM:SS" with optional fractional seconds, to milliseconds.
fn parse_timestamp(text: &str) -> Option<u64> {
    let mut parts = text.trim().split(':');
    let (hours, minutes, seconds) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let millis: u64 = format!("{fraction:0<3}")[..3].parse().ok()?;
    let hours: u64 = hours.parse().ok()?;
    let minutes: u64 = minutes.parse().ok().filter(|m| *m < 60)?;
    let seconds: u64 = whole.parse().ok().filter(|s| *s < 60)?;
    hours
        .checked_mul(3_600)?
        .checked_add(minutes * 60 + seconds)?
        .checked_mul(1_000)?
        .checked_add(millis)
}

// ------------------------------------------------------------------- MP4

/// Both ways MP4 files carry chapters are read, and the QuickTime chapter track
/// wins if it yields any. Apple's players (iTunes, Books, QuickTime) show that
/// track and ignore Nero's `chpl` list, so it is what a listener sees there;
/// `chpl` is the convention other players and tools fall back on, and files
/// from tools like ffmpeg carry both.
fn mp4_chapters(path: &Path) -> Result<Found> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let Some(moov) = read_moov(&mut file, length)? else {
        return Ok(Found {
            chapters: Vec::new(),
            duration_ms: None,
        });
    };
    let duration_ms = child(&moov, b"mvhd").and_then(movie_duration_ms);
    let mut chapters = quicktime_chapters(&mut file, length, &moov);
    if chapters.is_empty() {
        chapters = nero_chapters(&moov);
    }
    Ok(Found {
        chapters,
        duration_ms,
    })
}

/// The payload of the top-level `moov` box, found by hopping over the (huge)
/// `mdat` rather than reading it.
fn read_moov(file: &mut File, length: u64) -> io::Result<Option<Vec<u8>>> {
    let mut position = 0u64;
    while position + 8 <= length {
        file.seek(SeekFrom::Start(position))?;
        let mut header = [0u8; 16];
        let available = usize::try_from(length - position).unwrap_or(16).min(16);
        file.read_exact(&mut header[..available])?;
        let small = u64::from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]));
        let (header_len, size) = match small {
            1 if available == 16 => (
                16,
                u64::from_be_bytes([
                    header[8], header[9], header[10], header[11], header[12], header[13],
                    header[14], header[15],
                ]),
            ),
            1 => return Ok(None),
            0 => (8, length - position),
            size => (8, size),
        };
        if size < header_len || position + size > length {
            return Ok(None);
        }
        if &header[4..8] == b"moov" {
            let payload = size - header_len;
            if payload > MAX_MOOV_BYTES {
                return Ok(None);
            }
            file.seek(SeekFrom::Start(position + header_len))?;
            let mut moov = vec![0u8; payload as usize];
            file.read_exact(&mut moov)?;
            return Ok(Some(moov));
        }
        position += size;
    }
    Ok(None)
}

/// The boxes in a buffer, stopping at the first that is damaged or overruns it.
fn atoms(data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> + '_ {
    let mut rest = data;
    std::iter::from_fn(move || {
        let head = rest.get(..8)?;
        let small = u32::from_be_bytes(head[..4].try_into().ok()?) as usize;
        let kind: [u8; 4] = head[4..8].try_into().ok()?;
        let (header, size) = match small {
            1 => (
                16,
                usize::try_from(u64::from_be_bytes(rest.get(8..16)?.try_into().ok()?)).ok()?,
            ),
            0 => (8, rest.len()),
            size => (8, size),
        };
        if size < header || size > rest.len() {
            return None;
        }
        let payload = &rest[header..size];
        rest = &rest[size..];
        Some((kind, payload))
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    atoms(data)
        .find(|(found, _)| found == kind)
        .map(|(_, payload)| payload)
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn be64(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// `mvhd`/`mdhd`: version and flags, creation and modification times, then the
/// timescale and duration (32-bit, or 64-bit in version 1).
fn header_times(data: &[u8]) -> Option<(u32, u64)> {
    match *data.first()? {
        0 => Some((be32(data, 12)?, u64::from(be32(data, 16)?))),
        1 => Some((be32(data, 20)?, be64(data, 24)?)),
        _ => None,
    }
}

fn movie_duration_ms(mvhd: &[u8]) -> Option<u64> {
    let (timescale, duration) = header_times(mvhd)?;
    ticks_to_ms(duration, timescale).filter(|ms| *ms > 0)
}

fn ticks_to_ms(ticks: u64, timescale: u32) -> Option<u64> {
    u64::try_from(u128::from(ticks) * 1_000 / u128::from(timescale.max(1))).ok()
}

/// Nero's `chpl`: version and flags, four reserved bytes, a one-byte count,
/// then per chapter a start in 100 ns units and a length-prefixed UTF-8 title.
fn nero_chapters(moov: &[u8]) -> Vec<Raw> {
    let Some(chpl) = child(moov, b"udta").and_then(|udta| child(udta, b"chpl")) else {
        return Vec::new();
    };
    let Some(&count) = chpl.get(8) else {
        return Vec::new();
    };
    let mut chapters = Vec::new();
    let mut position = 9;
    for _ in 0..count {
        let Some(start) = be64(chpl, position) else {
            break;
        };
        let Some(&length) = chpl.get(position + 8) else {
            break;
        };
        let Some(title) = chpl.get(position + 9..position + 9 + usize::from(length)) else {
            break;
        };
        chapters.push(Raw {
            title: String::from_utf8_lossy(title).into_owned(),
            start_ms: start / 10_000,
            end_ms: None,
        });
        position += 9 + usize::from(length);
    }
    chapters
}

/// A track named by another track's `tref`/`chap` reference holds the chapters
/// as timed text: each sample's duration is a chapter's length and its text the
/// title.
fn quicktime_chapters(file: &mut File, file_length: u64, moov: &[u8]) -> Vec<Raw> {
    let tracks: Vec<&[u8]> = atoms(moov)
        .filter(|(kind, _)| kind == b"trak")
        .map(|(_, payload)| payload)
        .collect();
    let referenced: HashSet<u32> = tracks
        .iter()
        .filter_map(|track| child(track, b"tref"))
        .filter_map(|tref| child(tref, b"chap"))
        .flat_map(|ids| ids.chunks_exact(4).filter_map(|id| be32(id, 0)))
        .collect();
    for track in tracks {
        let id = child(track, b"tkhd").and_then(|tkhd| match *tkhd.first()? {
            0 => be32(tkhd, 12),
            1 => be32(tkhd, 20),
            _ => None,
        });
        if id.is_some_and(|id| referenced.contains(&id)) {
            let chapters = text_track_chapters(file, file_length, track);
            if !chapters.is_empty() {
                return chapters;
            }
        }
    }
    Vec::new()
}

fn text_track_chapters(file: &mut File, file_length: u64, track: &[u8]) -> Vec<Raw> {
    let Some(mdia) = child(track, b"mdia") else {
        return Vec::new();
    };
    let Some((timescale, _)) = child(mdia, b"mdhd").and_then(header_times) else {
        return Vec::new();
    };
    let Some(stbl) = child(mdia, b"minf").and_then(|minf| child(minf, b"stbl")) else {
        return Vec::new();
    };
    let (Some(stts), Some(stsz), Some(stsc)) = (
        child(stbl, b"stts"),
        child(stbl, b"stsz"),
        child(stbl, b"stsc"),
    ) else {
        return Vec::new();
    };
    let chunk_offsets = child(stbl, b"stco")
        .map(|stco| (stco, 4))
        .or_else(|| child(stbl, b"co64").map(|co64| (co64, 8)));
    let Some((chunks, offset_width)) = chunk_offsets else {
        return Vec::new();
    };

    let starts_and_lengths = sample_times(stts);
    let offsets = sample_offsets(stsc, chunks, offset_width, sample_sizes(stsz));
    let mut chapters = Vec::new();
    for ((start, length), (offset, size)) in starts_and_lengths.zip(offsets).take(MAX_CHAPTERS) {
        let Some(title) = read_text_sample(file, file_length, offset, size) else {
            continue;
        };
        let (Some(start_ms), Some(end_ms)) = (
            ticks_to_ms(start, timescale),
            ticks_to_ms(start + length, timescale),
        ) else {
            break;
        };
        chapters.push(Raw {
            title,
            start_ms,
            end_ms: Some(end_ms),
        });
    }
    chapters
}

/// Each sample's start and length in the track's ticks, from `stts` runs of
/// (count, duration). A run claiming billions of samples is only followed as
/// far as chapters are wanted.
fn sample_times(stts: &[u8]) -> impl Iterator<Item = (u64, u64)> + '_ {
    let runs = be32(stts, 4).unwrap_or(0) as usize;
    let mut run = 0;
    let mut left_in_run = 0u32;
    let mut delta = 0u64;
    let mut clock = 0u64;
    std::iter::from_fn(move || {
        while left_in_run == 0 {
            if run >= runs {
                return None;
            }
            left_in_run = be32(stts, 8 + run * 8)?;
            delta = u64::from(be32(stts, 12 + run * 8)?);
            run += 1;
        }
        left_in_run -= 1;
        let start = clock;
        clock = clock.checked_add(delta)?;
        Some((start, delta))
    })
}

/// Sample sizes from `stsz`: one fixed size, or a table.
fn sample_sizes(stsz: &[u8]) -> impl Iterator<Item = u32> + '_ {
    let fixed = be32(stsz, 4).unwrap_or(0);
    let count = be32(stsz, 8).unwrap_or(0) as usize;
    (0..count.min(MAX_CHAPTERS)).map_while(move |index| {
        if fixed != 0 {
            Some(fixed)
        } else {
            be32(stsz, 12 + index * 4)
        }
    })
}

/// Where each sample sits in the file: chunks hold runs of samples (`stsc`),
/// and chunk offsets come from `stco`/`co64`.
fn sample_offsets<'a>(
    stsc: &'a [u8],
    chunks: &'a [u8],
    offset_width: usize,
    sizes: impl Iterator<Item = u32> + 'a,
) -> impl Iterator<Item = (u64, u32)> + 'a {
    let runs = be32(stsc, 4).unwrap_or(0) as usize;
    let chunk_count = be32(chunks, 4).unwrap_or(0) as usize;
    let mut sizes = sizes;
    let mut chunk = 0usize;
    let mut left_in_chunk = 0u32;
    let mut position = 0u64;
    std::iter::from_fn(move || {
        while left_in_chunk == 0 {
            if chunk >= chunk_count {
                return None;
            }
            // The run in force is the last whose first chunk (1-based) has been reached.
            let mut per_chunk = 0;
            for run in 0..runs {
                let first = be32(stsc, 8 + run * 12)? as usize;
                if first <= chunk + 1 {
                    per_chunk = be32(stsc, 12 + run * 12)?;
                }
            }
            position = if offset_width == 4 {
                u64::from(be32(chunks, 8 + chunk * 4)?)
            } else {
                be64(chunks, 8 + chunk * 8)?
            };
            left_in_chunk = per_chunk;
            chunk += 1;
            if per_chunk == 0 {
                return None;
            }
        }
        left_in_chunk -= 1;
        let size = sizes.next()?;
        let offset = position;
        position = position.checked_add(u64::from(size))?;
        Some((offset, size))
    })
}

/// A text sample: a 16-bit length, then that many bytes of UTF-8 (or UTF-16
/// with a byte-order mark), then possibly style data that is not wanted.
fn read_text_sample(file: &mut File, file_length: u64, offset: u64, size: u32) -> Option<String> {
    if !(2..=MAX_TEXT_SAMPLE_BYTES).contains(&size)
        || offset.checked_add(u64::from(size))? > file_length
    {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut sample = vec![0u8; size as usize];
    file.read_exact(&mut sample).ok()?;
    let length = usize::from(u16::from_be_bytes([sample[0], sample[1]])).min(sample.len() - 2);
    let text = &sample[2..2 + length];
    Some(match text {
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        _ => String::from_utf8_lossy(text)
            .trim_end_matches('\0')
            .to_owned(),
    })
}

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| unit([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
        .trim_end_matches('\0')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use lofty::TextEncoding;
    use lofty::id3::v2::{
        ChapterFrame, ChapterTableOfContentsFrame, CtocFlags, FrameId, TextInformationFrame,
    };

    use super::*;

    fn raw(title: &str, start_ms: u64, end_ms: Option<u64>) -> Raw {
        Raw {
            title: title.to_owned(),
            start_ms,
            end_ms,
        }
    }

    fn summary(chapters: &[Chapter]) -> Vec<(&str, u64, Option<u64>)> {
        chapters
            .iter()
            .map(|c| (c.title.as_str(), c.start_ms, c.end_ms))
            .collect()
    }

    #[test]
    fn vorbis_timestamps_parse_to_milliseconds() {
        for (text, ms) in [
            ("00:00:00.000", 0),
            ("01:02:03.456", 3_723_456),
            ("00:00:01", 1_000),
            ("00:00:01.5", 1_500),
            ("00:00:01.12345", 1_123),
            (" 00:00:02.000 ", 2_000),
            ("100:00:00.000", 360_000_000),
        ] {
            assert_eq!(parse_timestamp(text), Some(ms), "{text:?}");
        }
        for text in [
            "",
            "1:2",
            "00:60:00",
            "00:00:60",
            "a:b:c",
            "00:00:01.x",
            "00:00:00:00",
            "-1:00:00",
            "00:00:1e3",
            "99999999999999999999:00:00",
        ] {
            assert_eq!(parse_timestamp(text), None, "{text:?}");
        }
    }

    #[test]
    fn finishing_drops_chapters_past_the_audio_and_holds_ends_to_it() {
        let chapters = finish(
            vec![
                raw("a", 0, Some(2_000)),
                raw("b", 2_000, Some(999_999)),
                raw("c", 6_000, None),
                raw("d", 9_000, Some(9_500)),
            ],
            6_000,
        );

        assert_eq!(
            summary(&chapters),
            [("a", 0, Some(2_000)), ("b", 2_000, Some(6_000))]
        );
        // Nothing is known about the length: nothing is dropped or invented.
        let unbounded = finish(vec![raw("a", 0, None), raw("b", 9_000, None)], 0);
        assert_eq!(
            summary(&unbounded),
            [("a", 0, Some(9_000)), ("b", 9_000, None)]
        );
    }

    #[test]
    fn missing_ends_come_from_the_next_chapter_or_the_end_of_the_audio() {
        let chapters = finish(
            vec![
                raw("a", 0, None),
                raw("b", 1_000, Some(500)),
                raw("c", 3_000, Some(3_000)),
                raw("d", 3_000, None),
            ],
            8_000,
        );

        assert_eq!(
            summary(&chapters),
            [
                ("a", 0, Some(1_000)),
                // An end before the start is no end.
                ("b", 1_000, Some(3_000)),
                ("c", 3_000, None),
                ("d", 3_000, Some(8_000)),
            ]
        );
    }

    #[test]
    fn titles_are_cleaned_capped_and_defaulted() {
        let long = "x".repeat(MAX_TITLE_CHARS + 100);
        let chapters = finish(
            vec![
                raw("  Padded\u{7}\ttitle  ", 0, None),
                raw("", 1_000, None),
                raw("   ", 2_000, None),
                raw(&long, 3_000, None),
            ],
            10_000,
        );

        assert_eq!(chapters[0].title, "Paddedtitle");
        assert_eq!(chapters[1].title, "Chapter 2");
        assert_eq!(chapters[2].title, "Chapter 3");
        assert_eq!(chapters[3].title.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn the_chapter_count_is_capped() {
        let many: Vec<Raw> = (0..MAX_CHAPTERS as u64 + 5_000)
            .map(|n| raw("c", n, None))
            .collect();

        assert_eq!(finish(many, 0).len(), MAX_CHAPTERS);
    }

    fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend(kind);
        bytes.extend(payload);
        bytes
    }

    #[test]
    fn boxes_are_walked_with_every_size_form_and_stop_when_damaged() {
        let mut data = atom(b"aaaa", b"one");
        let mut large = 1u32.to_be_bytes().to_vec();
        large.extend(b"bbbb");
        large.extend(19u64.to_be_bytes());
        large.extend(b"two");
        data.extend(large);
        let mut to_end = 0u32.to_be_bytes().to_vec();
        to_end.extend(b"cccc");
        to_end.extend(b"three");
        data.extend(to_end);

        let found: Vec<_> = atoms(&data)
            .map(|(kind, payload)| (kind, payload.to_vec()))
            .collect();
        assert_eq!(
            found,
            [
                (*b"aaaa", b"one".to_vec()),
                (*b"bbbb", b"two".to_vec()),
                (*b"cccc", b"three".to_vec())
            ]
        );

        let mut bad = atom(b"good", b"ok");
        bad.extend(100u32.to_be_bytes());
        bad.extend(b"liar");
        bad.extend(b"short");
        assert_eq!(
            atoms(&bad).count(),
            1,
            "a box that overruns the data ends the walk"
        );
        let mut tiny = 4u32.to_be_bytes().to_vec();
        tiny.extend(b"tiny");
        assert_eq!(
            atoms(&tiny).count(),
            0,
            "a box smaller than its own header is damaged"
        );
        assert_eq!(atoms(&[1, 2, 3]).count(), 0);
    }

    fn chpl(entries: &[(u64, &str)], declared: u8) -> Vec<u8> {
        let mut payload = vec![1, 0, 0, 0, 0, 0, 0, 0, declared];
        for (start, title) in entries {
            payload.extend(start.to_be_bytes());
            payload.push(title.len() as u8);
            payload.extend(title.as_bytes());
        }
        atom(b"udta", &atom(b"chpl", &payload))
    }

    #[test]
    fn nero_chapter_lists_convert_from_100_nanosecond_units() {
        let moov = chpl(
            &[
                (0, "Start"),
                (25_000_000, "Two and a half seconds"),
                (36_000_000_000, "An hour"),
            ],
            3,
        );

        let chapters = nero_chapters(&moov);

        assert_eq!(
            chapters
                .iter()
                .map(|c| (c.title.as_str(), c.start_ms))
                .collect::<Vec<_>>(),
            [
                ("Start", 0),
                ("Two and a half seconds", 2_500),
                ("An hour", 3_600_000)
            ]
        );
    }

    #[test]
    fn a_damaged_nero_list_keeps_what_came_before_the_damage() {
        let lying = chpl(&[(0, "One"), (10_000_000, "Two")], 255);
        assert_eq!(nero_chapters(&lying).len(), 2, "a count beyond the data");

        let mut cut = chpl(
            &[(0, "One"), (10_000_000, "A longer title that is cut off")],
            2,
        );
        cut.truncate(cut.len() - 10);
        let mut fixed = cut.clone();
        // Keep the box sizes consistent with the shortened data.
        let total = fixed.len() as u32;
        fixed[..4].copy_from_slice(&total.to_be_bytes());
        let inner = (fixed.len() - 8) as u32;
        fixed[8..12].copy_from_slice(&inner.to_be_bytes());
        assert_eq!(
            nero_chapters(&fixed).len(),
            1,
            "a title running past the end"
        );

        assert!(nero_chapters(&atom(b"udta", &atom(b"chpl", &[1, 0, 0]))).is_empty());
        assert!(nero_chapters(&atom(b"udta", &[])).is_empty());
        assert!(nero_chapters(&[]).is_empty());
    }

    #[test]
    fn time_to_sample_runs_are_followed_lazily() {
        let mut stts = vec![0, 0, 0, 0];
        stts.extend(3u32.to_be_bytes());
        for (count, delta) in [(2u32, 1_000u32), (4_000_000_000, 500), (1, 7)] {
            stts.extend(count.to_be_bytes());
            stts.extend(delta.to_be_bytes());
        }

        let first: Vec<_> = sample_times(&stts).take(4).collect();

        assert_eq!(
            first,
            [(0, 1_000), (1_000, 1_000), (2_000, 500), (2_500, 500)]
        );
        // Taking a handful from four billion samples does not allocate them.
        assert_eq!(sample_times(&stts).take(MAX_CHAPTERS).count(), MAX_CHAPTERS);
    }

    fn table(entries: &[u32]) -> Vec<u8> {
        let mut bytes = vec![0, 0, 0, 0];
        bytes.extend((entries.len() as u32).to_be_bytes());
        bytes
    }

    #[test]
    fn samples_are_located_through_chunks_and_runs() {
        // Two samples per chunk for the first two chunks, then one per chunk.
        let mut stsc = table(&[]);
        stsc[4..8].copy_from_slice(&2u32.to_be_bytes());
        for (first, per_chunk, description) in [(1u32, 2u32, 1u32), (3, 1, 1)] {
            stsc.extend(first.to_be_bytes());
            stsc.extend(per_chunk.to_be_bytes());
            stsc.extend(description.to_be_bytes());
        }
        let mut stco = table(&[100, 200, 300, 400]);
        for offset in [100u32, 200, 300, 400] {
            stco.extend(offset.to_be_bytes());
        }
        let sizes = [10u32, 20, 30, 40, 50];

        let located: Vec<_> = sample_offsets(&stsc, &stco, 4, sizes.into_iter()).collect();

        assert_eq!(
            located,
            [(100, 10), (110, 20), (200, 30), (230, 40), (300, 50)]
        );
    }

    #[test]
    fn a_fixed_sample_size_applies_to_every_sample() {
        let mut stsz = vec![0, 0, 0, 0];
        stsz.extend(12u32.to_be_bytes());
        stsz.extend(3u32.to_be_bytes());

        assert_eq!(sample_sizes(&stsz).collect::<Vec<_>>(), [12, 12, 12]);
        let mut huge = vec![0, 0, 0, 0];
        huge.extend(12u32.to_be_bytes());
        huge.extend(u32::MAX.to_be_bytes());
        assert_eq!(sample_sizes(&huge).count(), MAX_CHAPTERS);
    }

    #[test]
    fn text_samples_decode_in_each_encoding_and_refuse_bad_sizes() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("samples.bin");
        let mut file_bytes = Vec::new();
        let mut put = |bytes: &[u8]| {
            let offset = file_bytes.len() as u64;
            file_bytes.extend(bytes);
            (offset, bytes.len() as u32)
        };
        let utf8 = put(&[&[0, 5][..], b"Intro", b"styl"].concat());
        let be = put(&[
            &[0, 8][..],
            &[0xFE, 0xFF, 0x00, b'H', 0x00, b'i', 0x00, 0xE9][..],
        ]
        .concat());
        let le = put(&[
            &[0, 8][..],
            &[0xFF, 0xFE, b'H', 0x00, b'i', 0x00, 0xE9, 0x00][..],
        ]
        .concat());
        let nul = put(&[&[0, 4][..], b"End\0"].concat());
        let overlong = put(&[&[0xFF, 0xFF][..], b"abc"].concat());
        let tiny = put(&[7]);
        std::fs::write(&path, &file_bytes).unwrap();
        let mut file = File::open(&path).unwrap();
        let length = file_bytes.len() as u64;
        let mut read =
            |(offset, size): (u64, u32)| read_text_sample(&mut file, length, offset, size);

        assert_eq!(read(utf8).as_deref(), Some("Intro"));
        assert_eq!(read(be).as_deref(), Some("Hi\u{e9}"));
        assert_eq!(read(le).as_deref(), Some("Hi\u{e9}"));
        assert_eq!(read(nul).as_deref(), Some("End"));
        assert_eq!(
            read(overlong).as_deref(),
            Some("abc"),
            "a length beyond the sample is clamped"
        );
        assert_eq!(read(tiny), None);
        assert_eq!(
            read((length - 1, 50)),
            None,
            "a sample running past the file"
        );
        assert_eq!(read((length + 100, 8)), None);
        assert_eq!(read((0, MAX_TEXT_SAMPLE_BYTES + 1)), None);
    }

    fn chap(id: &str, start: u32, end: u32, title: Option<&str>) -> Frame<'static> {
        let mut children = lofty::id3::v2::FrameList::new();
        if let Some(title) = title {
            children.insert(Frame::Text(TextInformationFrame::new(
                FrameId::new("TIT2").unwrap(),
                TextEncoding::UTF8,
                title.to_owned(),
            )));
        }
        Frame::Chapter(ChapterFrame::new(
            id.to_owned(),
            start..end,
            u32::MAX..u32::MAX,
            children,
        ))
    }

    fn ctoc(id: &str, top_level: bool, ordered: bool, entries: &[&str]) -> Frame<'static> {
        let entries: Vec<std::borrow::Cow<'static, str>> = entries
            .iter()
            .map(|e| std::borrow::Cow::Owned((*e).to_owned()))
            .collect();
        Frame::TableOfContents(ChapterTableOfContentsFrame::new(
            id.to_owned(),
            CtocFlags { top_level, ordered },
            entries,
            lofty::id3::v2::FrameList::new(),
        ))
    }

    fn tag_with(frames: Vec<Frame<'static>>) -> Id3v2Tag {
        let mut tag = Id3v2Tag::new();
        for frame in frames {
            tag.insert(frame);
        }
        tag
    }

    fn titles(raw: &[Raw]) -> Vec<&str> {
        raw.iter().map(|r| r.title.as_str()).collect()
    }

    #[test]
    fn id3_chapters_without_a_table_of_contents_go_by_start_time() {
        let tag = tag_with(vec![
            chap("c", 9_000, 12_000, Some("Third")),
            chap("a", 0, 4_000, Some("First")),
            chap("b", 4_000, 4_000, Some("Second")),
        ]);

        let found = id3_chapters(&tag);

        assert_eq!(titles(&found), ["First", "Second", "Third"]);
        assert_eq!(found[0].end_ms, Some(4_000));
        assert_eq!(found[1].end_ms, None, "an end equal to the start is no end");
    }

    #[test]
    fn an_ordered_table_of_contents_sets_the_order() {
        let tag = tag_with(vec![
            chap("a", 0, 1_000, Some("A")),
            chap("b", 1_000, 2_000, Some("B")),
            chap("c", 2_000, 3_000, Some("C")),
            chap("loose", 500, 600, Some("Loose")),
            ctoc("toc", true, true, &["c", "a", "b"]),
        ]);

        assert_eq!(
            titles(&id3_chapters(&tag)),
            ["C", "A", "B", "Loose"],
            "unlisted chapters follow"
        );
    }

    #[test]
    fn an_unordered_table_of_contents_still_lists_by_time() {
        let tag = tag_with(vec![
            chap("a", 0, 1_000, Some("A")),
            chap("b", 1_000, 2_000, Some("B")),
            chap("c", 2_000, 3_000, Some("C")),
            ctoc("toc", true, false, &["c", "a", "b"]),
        ]);

        assert_eq!(titles(&id3_chapters(&tag)), ["A", "B", "C"]);
    }

    #[test]
    fn nested_tables_are_followed_and_loops_are_not() {
        let tag = tag_with(vec![
            chap("a", 0, 1_000, Some("A")),
            chap("b", 1_000, 2_000, Some("B")),
            chap("c", 2_000, 3_000, Some("C")),
            ctoc("top", true, true, &["part2", "a"]),
            ctoc("part2", false, true, &["c", "top", "b", "part2"]),
        ]);

        assert_eq!(titles(&id3_chapters(&tag)), ["C", "B", "A"]);
    }

    #[test]
    fn a_chapter_without_a_title_frame_has_an_empty_title_for_the_default_to_fill() {
        let found = id3_chapters(&tag_with(vec![chap("a", 0, 1_000, None)]));

        assert_eq!(titles(&found), [""]);
        assert_eq!(finish(found, 5_000)[0].title, "Chapter 1");
    }
}
