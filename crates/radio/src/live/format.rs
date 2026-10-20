//! What kind of audio a stream carries, from its first bytes and, failing that, its content type.
//!
//! Servers are not trusted about this: many send `audio/mpeg` for AAC, so the bytes win.

use serde::{Deserialize, Serialize};

/// The container or codec a decoder should expect, as a hint for the playback engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Mp3,
    /// AAC in ADTS frames, the format of Icecast/SHOUTcast AAC streams.
    AacAdts,
    /// Ogg holding Vorbis (or anything but Opus).
    Ogg,
    /// Ogg holding Opus.
    Opus,
    /// Native FLAC (`fLaC`), as Icecast serves lossless streams.
    Flac,
    /// MPEG transport stream, as HLS usually carries audio.
    MpegTs,
    /// Fragmented MP4: an initialisation segment followed by fragments.
    Fmp4,
}

impl Format {
    pub fn mime(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::AacAdts => "audio/aac",
            Self::Ogg => "audio/ogg",
            Self::Opus => "audio/opus",
            Self::Flac => "audio/flac",
            Self::MpegTs => "video/mp2t",
            Self::Fmp4 => "audio/mp4",
        }
    }
}

/// The format a `Content-Type` names, if it names one this crate yields.
pub(crate) fn from_media_type(media_type: &str) -> Option<Format> {
    Some(match media_type {
        "audio/mpeg" | "audio/mp3" | "audio/mpg" | "audio/x-mpeg" | "audio/x-mp3" => Format::Mp3,
        "audio/aac" | "audio/aacp" | "audio/x-aac" | "audio/aac+" | "audio/x-aacp" => {
            Format::AacAdts
        }
        "audio/ogg" | "application/ogg" | "audio/x-ogg" | "audio/vorbis" => Format::Ogg,
        "audio/opus" => Format::Opus,
        "audio/flac" | "audio/x-flac" => Format::Flac,
        "video/mp2t" | "video/mpeg2" => Format::MpegTs,
        _ => return None,
    })
}

/// A transport stream packet is 188 bytes and starts with 0x47, so two of them in a row are
/// recognisable.
const TS_PACKET: usize = 188;
/// How far into the audio to look for a first frame, after any tag.
const SCAN_BYTES: usize = 4096;

/// The format of `bytes`, the start of a stream. `None` when they do not say (yet).
pub(crate) fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"OggS") {
        return Some(ogg_codec(bytes));
    }
    if bytes.starts_with(b"fLaC") {
        return Some(Format::Flac);
    }
    if bytes.len() >= 12
        && matches!(
            &bytes[4..8],
            b"ftyp" | b"styp" | b"moof" | b"moov" | b"sidx"
        )
    {
        return Some(Format::Fmp4);
    }
    if bytes.len() > TS_PACKET && bytes[0] == 0x47 && bytes[TS_PACKET] == 0x47 {
        return Some(Format::MpegTs);
    }

    // A tag may come first, as ID3v2 does at the start of an MP3 stream or an ADTS segment.
    let start = if bytes.starts_with(b"ID3") {
        id3_len(bytes)?
    } else {
        0
    };
    if start >= bytes.len() {
        return None;
    }
    let audio = &bytes[start..];
    // After a tag the frame is where the tag ends; in a stream joined mid-way it is wherever the
    // next one starts.
    let scan = if start > 0 {
        1
    } else {
        audio.len().min(SCAN_BYTES)
    };
    (0..scan).find_map(|at| frame_format(&audio[at..]))
}

fn ogg_codec(page: &[u8]) -> Format {
    // 27 header bytes, a segment table, then the first packet, which names the codec.
    let payload = page
        .get(26)
        .and_then(|&segments| page.get(27 + usize::from(segments)..));
    match payload {
        Some(payload) if payload.starts_with(b"OpusHead") => Format::Opus,
        _ => Format::Ogg,
    }
}

/// The size of an ID3v2 tag including its header (and footer), or `None` if the header is cut off.
pub(crate) fn id3_len(bytes: &[u8]) -> Option<usize> {
    let header = bytes.get(..10)?;
    let size = header[6..10]
        .iter()
        .fold(0usize, |size, &byte| (size << 7) | usize::from(byte & 0x7F));
    let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
    Some(10 + size + footer)
}

/// Whether a valid MP3 or ADTS frame starts here, and is followed by another where that can be seen.
fn frame_format(bytes: &[u8]) -> Option<Format> {
    let check = |length: Option<usize>, valid_next: fn(&[u8]) -> Option<usize>, format| {
        let length = length?;
        match bytes.get(length..) {
            Some(rest) if rest.len() >= 7 => valid_next(rest).map(|_| format),
            // The next header is beyond what has been read: one good frame will do.
            _ => Some(format),
        }
    };
    check(adts_len(bytes), adts_len, Format::AacAdts)
        .or_else(|| check(mp3_len(bytes), mp3_len, Format::Mp3))
}

/// The length of the ADTS frame at the start of `bytes`.
fn adts_len(bytes: &[u8]) -> Option<usize> {
    let header = bytes.get(..7)?;
    if header[0] != 0xFF || header[1] & 0xF6 != 0xF0 || (header[2] >> 2) & 0x0F >= 13 {
        return None;
    }
    let length = (usize::from(header[3] & 3) << 11)
        | (usize::from(header[4]) << 3)
        | usize::from(header[5] >> 5);
    (length >= 7).then_some(length)
}

const MPEG1_L1: [u32; 15] = [
    0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
];
const MPEG1_L2: [u32; 15] = [
    0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
];
const MPEG1_L3: [u32; 15] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];
const MPEG2_L1: [u32; 15] = [
    0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
];
const MPEG2_L23: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

/// The length of the MPEG audio frame at the start of `bytes`.
fn mp3_len(bytes: &[u8]) -> Option<usize> {
    let header = bytes.get(..4)?;
    if header[0] != 0xFF || header[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (header[1] >> 3) & 3; // 0 = 2.5, 1 = reserved, 2 = 2, 3 = 1
    let layer = (header[1] >> 1) & 3; // 1 = III, 2 = II, 3 = I
    let bitrate_index = usize::from(header[2] >> 4);
    let rate_index = usize::from((header[2] >> 2) & 3);
    if version == 1 || layer == 0 || bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
        return None;
    }
    let mpeg1 = version == 3;
    let table = match (mpeg1, layer) {
        (true, 3) => &MPEG1_L1,
        (true, 2) => &MPEG1_L2,
        (true, _) => &MPEG1_L3,
        (false, 3) => &MPEG2_L1,
        (false, _) => &MPEG2_L23,
    };
    let sample_rate = [44_100u32, 48_000, 32_000][rate_index]
        >> match version {
            3 => 0,
            2 => 1,
            _ => 2,
        };
    let bitrate = table[bitrate_index] * 1000;
    let padding = u32::from((header[2] >> 1) & 1);
    let length = match (layer, mpeg1) {
        (3, _) => (12 * bitrate / sample_rate + padding) * 4,
        (_, false) if layer == 1 => 72 * bitrate / sample_rate + padding,
        _ => 144 * bitrate / sample_rate + padding,
    };
    usize::try_from(length).ok().filter(|&length| length >= 4)
}

#[cfg(test)]
pub(crate) mod samples {
    //! Hand-made frames for tests.

    /// An MPEG-1 Layer III frame at 128 kbps and 44.1 kHz: 417 bytes, 418 with padding.
    pub fn mp3_frame(padding: bool) -> Vec<u8> {
        let length = if padding { 418 } else { 417 };
        let mut frame = vec![0u8; length];
        frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90 | (u8::from(padding) << 1), 0x00]);
        frame
    }

    /// `range.len()` MPEG frames, each carrying its own number so a test can tell exactly which
    /// audio came through.
    pub fn numbered_mp3(range: std::ops::Range<u32>) -> Vec<u8> {
        range
            .flat_map(|number| {
                let mut frame = mp3_frame(false);
                frame[4..8].copy_from_slice(&number.to_be_bytes());
                frame
            })
            .collect()
    }

    /// An ADTS frame (AAC-LC, 44.1 kHz, stereo) of `length` bytes.
    pub fn adts_frame(length: usize) -> Vec<u8> {
        let mut frame = vec![0u8; length];
        frame[0] = 0xFF;
        frame[1] = 0xF1;
        frame[2] = 0x50; // AAC-LC, 44.1 kHz
        frame[3] = 0x80 | ((length >> 11) & 3) as u8;
        frame[4] = ((length >> 3) & 0xFF) as u8;
        frame[5] = (((length & 7) << 5) | 0x1F) as u8;
        frame[6] = 0xFC;
        frame
    }

    pub fn ogg_page(first_packet: &[u8]) -> Vec<u8> {
        let mut page = b"OggS".to_vec();
        page.extend([
            0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        page.push(1);
        page.push(u8::try_from(first_packet.len()).unwrap());
        page.extend(first_packet);
        page
    }

    pub fn id3v2(payload_size: usize) -> Vec<u8> {
        let mut tag = b"ID3\x03\x00\x00".to_vec();
        tag.extend([
            ((payload_size >> 21) & 0x7F) as u8,
            ((payload_size >> 14) & 0x7F) as u8,
            ((payload_size >> 7) & 0x7F) as u8,
            (payload_size & 0x7F) as u8,
        ]);
        tag.extend(vec![0u8; payload_size]);
        tag
    }

    pub fn ts_packets(count: usize) -> Vec<u8> {
        (0..count)
            .flat_map(|_| std::iter::once(0x47).chain([0u8; 187]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{samples::*, *};

    fn stream_of(frame: &[u8], count: usize) -> Vec<u8> {
        frame.repeat(count)
    }

    #[test]
    fn mp3_frames_are_recognised_and_measured() {
        assert_eq!(mp3_len(&mp3_frame(false)), Some(417));
        assert_eq!(mp3_len(&mp3_frame(true)), Some(418));
        assert_eq!(sniff(&stream_of(&mp3_frame(false), 4)), Some(Format::Mp3));
        // MPEG-2 Layer III at 64 kbps and 22.05 kHz: 72 * 64000 / 22050 = 208.
        assert_eq!(mp3_len(&[0xFF, 0xF3, 0x80, 0x00]), Some(208));
        // MPEG-1 Layer II at 192 kbps and 48 kHz: 144 * 192000 / 48000 = 576.
        assert_eq!(mp3_len(&[0xFF, 0xFD, 0xA4, 0x00]), Some(576));
    }

    #[test]
    fn invalid_mp3_headers_are_not_frames() {
        for header in [
            [0xFF, 0xE1, 0x90, 0x00], // reserved version
            [0xFF, 0xF9, 0x90, 0x00], // layer 0 is ADTS
            [0xFF, 0xFB, 0xF0, 0x00], // bitrate index 15
            [0xFF, 0xFB, 0x0C, 0x00], // free-format bitrate
            [0xFF, 0xFB, 0x9C, 0x00], // sample rate index 3
        ] {
            assert_eq!(mp3_len(&header), None, "{header:x?}");
        }
    }

    #[test]
    fn adts_frames_are_recognised_and_measured() {
        assert_eq!(adts_len(&adts_frame(300)), Some(300));
        assert_eq!(
            sniff(&stream_of(&adts_frame(300), 5)),
            Some(Format::AacAdts)
        );
        // A header claiming a frame shorter than itself is noise.
        let mut short = adts_frame(7);
        short[3] &= 0xFC;
        short[4] = 0;
        short[5] = (5 << 5) | 0x1F;
        assert_eq!(adts_len(&short), None);
    }

    #[test]
    fn a_stream_joined_mid_frame_is_found_by_scanning() {
        let mut stream = vec![0x12, 0x34, 0x56];
        stream.extend(stream_of(&mp3_frame(false), 3));
        assert_eq!(sniff(&stream), Some(Format::Mp3));
        let mut stream = vec![0xAA; 200];
        stream.extend(stream_of(&adts_frame(256), 3));
        assert_eq!(sniff(&stream), Some(Format::AacAdts));
    }

    #[test]
    fn a_false_sync_without_a_second_frame_is_not_enough() {
        // 0xFF 0xFB looks like a header, but nothing follows at the length it claims.
        let mut noise = vec![0u8; 1200];
        noise[10..14].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        assert_eq!(sniff(&noise), None);
    }

    #[test]
    fn ogg_pages_say_which_codec_they_carry() {
        assert_eq!(sniff(&ogg_page(b"OpusHead\x01\x02")), Some(Format::Opus));
        assert_eq!(sniff(&ogg_page(b"\x01vorbis\x00\x00")), Some(Format::Ogg));
        assert_eq!(sniff(&ogg_page(b"\x7fFLAC\x01\x00")), Some(Format::Ogg));
        assert_eq!(sniff(b"OggS"), Some(Format::Ogg));
    }

    #[test]
    fn flac_transport_streams_and_mp4_are_recognised() {
        assert_eq!(sniff(b"fLaC\0\0\0\"\x10\x00"), Some(Format::Flac));
        assert_eq!(sniff(&ts_packets(3)), Some(Format::MpegTs));
        assert_eq!(sniff(b"\0\0\0\x18ftypiso5\0\0\0\0"), Some(Format::Fmp4));
        assert_eq!(sniff(b"\0\0\0\x18styp"), None);
        assert_eq!(sniff(b"\0\0\0\x20styp\0\0\0\0iso6"), Some(Format::Fmp4));
    }

    #[test]
    fn a_leading_id3_tag_is_skipped() {
        let mut stream = id3v2(500);
        stream.extend(stream_of(&mp3_frame(false), 2));
        assert_eq!(sniff(&stream), Some(Format::Mp3));
        let mut stream = id3v2(60);
        stream.extend(stream_of(&adts_frame(200), 2));
        assert_eq!(sniff(&stream), Some(Format::AacAdts));
        // A tag that has not been read to its end says nothing yet.
        assert_eq!(sniff(&id3v2(500)[..100]), None);
        assert_eq!(sniff(b"ID3\x03"), None);
    }

    #[test]
    fn content_types_map_to_formats() {
        for (media_type, format) in [
            ("audio/mpeg", Format::Mp3),
            ("audio/aacp", Format::AacAdts),
            ("audio/aac", Format::AacAdts),
            ("application/ogg", Format::Ogg),
            ("audio/opus", Format::Opus),
            ("audio/x-flac", Format::Flac),
            ("video/mp2t", Format::MpegTs),
        ] {
            assert_eq!(from_media_type(media_type), Some(format), "{media_type}");
        }
        assert_eq!(from_media_type("text/html"), None);
    }

    #[test]
    fn formats_serialise_in_camel_case_and_have_mime_types() {
        assert_eq!(serde_json::to_value(Format::AacAdts).unwrap(), "aacAdts");
        assert_eq!(serde_json::to_value(Format::MpegTs).unwrap(), "mpegTs");
        assert_eq!(Format::Opus.mime(), "audio/opus");
    }
}
