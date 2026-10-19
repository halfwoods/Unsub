//! Encoder delay and padding for `.m4a` files.
//!
//! AAC encoders prepend priming samples and pad the last frame, and the container records how
//! much to discard so that gapless albums join up. symphonia 0.6 parses the edit list but does
//! not apply it and does not expose `iTunSMPB`, so both are read here.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// A moov atom larger than this is not a real index.
const MAX_MOOV_BYTES: u64 = 64 * 1024 * 1024;

/// How much of the decoded stream is audio, in the track's media-timescale ticks.
#[derive(Debug, PartialEq, Eq)]
pub struct Trim {
    pub start: u64,
    pub length: Option<u64>,
}

/// Looks up the trim for `track_id`. `None` means the file carries nothing usable.
pub fn read_trim(path: &Path, track_id: u32) -> Option<Trim> {
    let moov = read_moov(path)?;
    trim_from_moov(&moov, track_id)
}

fn read_moov(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    let mut offset = 0;
    while offset + 8 <= file_len {
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut header = [0; 16];
        let read = file.read(&mut header).ok()?;
        let (size, header_len) = match be_u32(&header, 0)? {
            0 => (file_len - offset, 8),
            1 => (be_u64(&header, 8)?, 16),
            size => (u64::from(size), 8),
        };
        if size < header_len || read < 8 {
            return None;
        }
        if header.get(4..8)? == b"moov" {
            let payload = size - header_len;
            if payload > MAX_MOOV_BYTES {
                return None;
            }
            file.seek(SeekFrom::Start(offset + header_len)).ok()?;
            let mut moov = vec![0; usize::try_from(payload).ok()?];
            file.read_exact(&mut moov).ok()?;
            return Some(moov);
        }
        offset += size;
    }
    None
}

fn trim_from_moov(moov: &[u8], track_id: u32) -> Option<Trim> {
    let trak = boxes(moov)
        .filter(|(kind, _)| kind == b"trak")
        .map(|(_, payload)| payload)
        .find(|trak| child(trak, b"tkhd").and_then(tkhd_track_id) == Some(track_id))?;
    let mdhd = child(child(trak, b"mdia")?, b"mdhd").and_then(parse_mdhd)?;

    // Apple's tag is exact to the sample, so it outranks the edit list.
    if let Some(trim) = itunes_trim(moov, mdhd.duration) {
        return Some(trim);
    }
    let movie_timescale = child(moov, b"mvhd").and_then(mvhd_timescale)?;
    edit_list_trim(child(trak, b"edts")?, movie_timescale, &mdhd)
}

struct Mdhd {
    timescale: u64,
    duration: u64,
}

/// The first (and for gapless audio, only) edit that plays media: start offset and length.
fn edit_list_trim(edts: &[u8], movie_timescale: u64, mdhd: &Mdhd) -> Option<Trim> {
    let elst = child(edts, b"elst")?;
    let version = *elst.first()?;
    let count = be_u32(elst, 4)?;
    let entry_len = if version == 1 { 20 } else { 12 };

    let mut media_edits = (0..count as usize).filter_map(|n| {
        let entry = elst.get(8 + n * entry_len..)?;
        let (duration, media_time) = if version == 1 {
            (be_u64(entry, 0)?, be_u64(entry, 8)? as i64)
        } else {
            (
                u64::from(be_u32(entry, 0)?),
                i64::from(be_u32(entry, 4)? as i32),
            )
        };
        // A media time of -1 is an empty edit: silence before the audio, not a trim.
        u64::try_from(media_time)
            .ok()
            .map(|start| (duration, start))
    });
    let (segment_duration, start) = media_edits.next()?;
    if media_edits.next().is_some() {
        return None;
    }

    // The track's own length is exact; the edit's is only as precise as the movie timescale
    // (ffmpeg uses milliseconds), so trust it only when it disagrees by more than that rounding.
    let rest = mdhd.duration.checked_sub(start)?;
    let from_edit = segment_duration * mdhd.timescale / movie_timescale.max(1);
    let rounding = mdhd.timescale / movie_timescale.max(1);
    let length = match segment_duration {
        0 => rest,
        _ if from_edit.abs_diff(rest) <= rounding => rest,
        _ => from_edit.min(rest),
    };
    Some(Trim {
        start,
        length: Some(length),
    })
}

/// Reads `iTunSMPB`: `" 00000000 <priming> <padding> <frames> ..."`, all hex.
fn itunes_trim(moov: &[u8], media_duration: u64) -> Option<Trim> {
    let ilst = child(
        child(child(moov, b"udta")?, b"meta").and_then(skip_full_box)?,
        b"ilst",
    )?;
    let value = boxes(ilst)
        .filter(|(kind, _)| kind == b"----")
        .find_map(|(_, item)| {
            let name = child(item, b"name")?.get(4..)?;
            (name == b"iTunSMPB").then(|| child(item, b"data")?.get(8..))?
        })?;

    let text = std::str::from_utf8(value).ok()?;
    let mut fields = text
        .split_whitespace()
        .map(|field| u64::from_str_radix(field, 16).ok());
    let (_, priming, padding, frames) = (
        fields.next()?,
        fields.next()??,
        fields.next()??,
        fields.next()??,
    );

    // Some encoders leave the frame count zero; the rest of the stream is then the audio.
    let length = match frames {
        0 => media_duration.checked_sub(priming)?.checked_sub(padding)?,
        frames => frames,
    };
    Some(Trim {
        start: priming,
        length: Some(length),
    })
}

/// Iterates the boxes in `data` as `(fourcc, payload)`.
fn boxes(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let kind: [u8; 4] = data.get(4..8)?.try_into().ok()?;
        let (header_len, size) = match be_u32(data, 0)? {
            0 => (8, data.len()),
            1 => (16, usize::try_from(be_u64(data, 8)?).ok()?),
            size => (8, size as usize),
        };
        let payload = data.get(header_len..size)?;
        data = data.get(size..)?;
        Some((kind, payload))
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data)
        .find(|(found, _)| found == kind)
        .map(|(_, payload)| payload)
}

/// `meta` is a full box: its children follow a version and flags word.
fn skip_full_box(payload: &[u8]) -> Option<&[u8]> {
    payload.get(4..)
}

fn tkhd_track_id(tkhd: &[u8]) -> Option<u32> {
    be_u32(tkhd, if *tkhd.first()? == 1 { 20 } else { 12 })
}

fn mvhd_timescale(mvhd: &[u8]) -> Option<u64> {
    be_u32(mvhd, if *mvhd.first()? == 1 { 20 } else { 12 }).map(u64::from)
}

fn parse_mdhd(mdhd: &[u8]) -> Option<Mdhd> {
    if *mdhd.first()? == 1 {
        Some(Mdhd {
            timescale: u64::from(be_u32(mdhd, 20)?),
            duration: be_u64(mdhd, 24)?,
        })
    } else {
        Some(Mdhd {
            timescale: u64::from(be_u32(mdhd, 12)?),
            duration: u64::from(be_u32(mdhd, 16)?),
        })
    }
}

fn be_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn be_u64(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn full(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        atom(kind, &[&[0; 4], body].concat())
    }

    fn mvhd(timescale: u32) -> Vec<u8> {
        let mut body = vec![0; 8];
        body.extend(timescale.to_be_bytes());
        body.extend(0u32.to_be_bytes());
        full(b"mvhd", &body)
    }

    fn tkhd(track_id: u32) -> Vec<u8> {
        let mut body = vec![0; 8];
        body.extend(track_id.to_be_bytes());
        full(b"tkhd", &body)
    }

    fn mdhd(timescale: u32, duration: u32) -> Vec<u8> {
        let mut body = vec![0; 8];
        body.extend(timescale.to_be_bytes());
        body.extend(duration.to_be_bytes());
        full(b"mdhd", &body)
    }

    fn edts(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut body = (entries.len() as u32).to_be_bytes().to_vec();
        for &(duration, media_time) in entries {
            body.extend(duration.to_be_bytes());
            body.extend(media_time.to_be_bytes());
            body.extend(0x0001_0000u32.to_be_bytes());
        }
        atom(b"edts", &full(b"elst", &body))
    }

    fn itunes_tag(value: &str) -> Vec<u8> {
        let mean = full(b"mean", b"com.apple.iTunes");
        let name = full(b"name", b"iTunSMPB");
        let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
        data.extend(value.as_bytes());
        let item = atom(b"----", &[mean, name, atom(b"data", &data)].concat());
        atom(b"udta", &full(b"meta", &atom(b"ilst", &item)))
    }

    fn moov(
        movie_timescale: u32,
        media: (u32, u32),
        edits: Option<&[(u32, i32)]>,
        tag: Option<&str>,
    ) -> Vec<u8> {
        let mdia = atom(b"mdia", &mdhd(media.0, media.1));
        let trak = atom(
            b"trak",
            &[tkhd(1), edits.map(edts).unwrap_or_default(), mdia].concat(),
        );
        [
            mvhd(movie_timescale),
            trak,
            tag.map(itunes_tag).unwrap_or_default(),
        ]
        .concat()
    }

    #[test]
    fn itunes_tag_gives_priming_and_exact_length() {
        let tag = " 00000000 00000840 000002F6 000000000000A8CA 00000000 00000000";
        let trim = trim_from_moov(&moov(44_100, (44_100, 46_080), None, Some(tag)), 1);
        assert_eq!(
            trim,
            Some(Trim {
                start: 2_112,
                length: Some(43_210)
            })
        );
    }

    #[test]
    fn itunes_tag_with_zero_frame_count_derives_the_length() {
        let tag = " 00000000 00000840 000002F6 0000000000000000";
        let trim = trim_from_moov(&moov(44_100, (44_100, 46_080), None, Some(tag)), 1);
        assert_eq!(
            trim,
            Some(Trim {
                start: 2_112,
                length: Some(46_080 - 2_112 - 758)
            })
        );
    }

    #[test]
    fn itunes_tag_outranks_the_edit_list() {
        let tag = " 00000000 00000840 000002F6 000000000000A8CA";
        let moov = moov(
            44_100,
            (44_100, 46_080),
            Some(&[(43_000, 1_024)]),
            Some(tag),
        );
        assert_eq!(trim_from_moov(&moov, 1).map(|trim| trim.start), Some(2_112));
    }

    #[test]
    fn coarse_edit_list_defers_to_the_exact_media_duration() {
        // ffmpeg: 43210 frames become 980 ms at the 1000 Hz movie timescale.
        let moov = moov(1_000, (44_100, 44_234), Some(&[(980, 1_024)]), None);
        assert_eq!(
            trim_from_moov(&moov, 1),
            Some(Trim {
                start: 1_024,
                length: Some(43_210)
            })
        );
    }

    #[test]
    fn edit_list_shorter_than_the_media_is_honoured() {
        let moov = moov(44_100, (44_100, 50_000), Some(&[(40_000, 1_024)]), None);
        assert_eq!(
            trim_from_moov(&moov, 1),
            Some(Trim {
                start: 1_024,
                length: Some(40_000)
            })
        );
    }

    #[test]
    fn empty_edits_are_not_trims_and_several_media_edits_are_ignored() {
        let leading_silence = moov(1_000, (44_100, 44_100), Some(&[(500, -1)]), None);
        assert_eq!(trim_from_moov(&leading_silence, 1), None);
        let spliced = moov(
            1_000,
            (44_100, 44_100),
            Some(&[(400, 0), (400, 20_000)]),
            None,
        );
        assert_eq!(trim_from_moov(&spliced, 1), None);
    }

    #[test]
    fn a_file_with_no_trim_information_has_none() {
        assert_eq!(
            trim_from_moov(&moov(1_000, (44_100, 44_100), None, None), 1),
            None
        );
    }

    #[test]
    fn another_track_id_finds_nothing() {
        let tag = " 00000000 00000840 000002F6 000000000000A8CA";
        assert_eq!(
            trim_from_moov(&moov(44_100, (44_100, 46_080), None, Some(tag)), 2),
            None
        );
    }

    #[test]
    fn truncated_and_garbage_input_is_rejected_without_panicking() {
        let good = moov(1_000, (44_100, 44_234), Some(&[(980, 1_024)]), None);
        for len in 0..good.len() {
            let _ = trim_from_moov(&good[..len], 1);
        }
        assert_eq!(trim_from_moov(&[0xFF; 64], 1), None);
        assert_eq!(trim_from_moov(&[], 1), None);
    }
}
