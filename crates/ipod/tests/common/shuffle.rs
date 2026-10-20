//! Test-only reader of iTunesSD files, written from the format as documented by iPodLinux and
//! gnupod, sharing nothing with the writer under test.

fn be24(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([0, bytes[at], bytes[at + 1], bytes[at + 2]])
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SdTrack {
    pub path: String,
    pub start: u32,
    pub stop: u32,
    pub volume: i64,
    pub file_type: u32,
    pub plays_when_shuffling: bool,
    pub bookmarkable: bool,
    pub bookmark_ms: u32,
    pub dbid: u64,
    pub album_id: u32,
    pub artist_id: u32,
    pub track_number: u16,
    pub disc_number: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SdPlaylist {
    pub kind: u32,
    pub id: u64,
    pub members: Vec<u32>,
    pub non_spoken_count: u32,
}

#[derive(Debug, Default)]
pub struct Sd {
    pub tracks: Vec<SdTrack>,
    pub playlists: Vec<SdPlaylist>,
    pub volume_limit: u8,
    pub voiceover: bool,
    pub non_spoken_tracks: u32,
}

/// The 1G and 2G layout: 18 bytes of header and 558 bytes per track, integers big endian.
pub fn parse_fixed(bytes: &[u8]) -> Sd {
    let count = be24(bytes, 0) as usize;
    assert_eq!(be24(bytes, 3), 0x01_0600);
    assert_eq!(be24(bytes, 6), 0x12);
    assert_eq!(
        bytes.len(),
        18 + 558 * count,
        "header plus fixed-size records"
    );
    let tracks = (0..count)
        .map(|index| {
            let record = &bytes[18 + 558 * index..18 + 558 * (index + 1)];
            assert_eq!(be24(record, 0), 0x22e);
            assert_eq!(be24(record, 3), 0x5a_a501);
            assert_eq!(be24(record, 30), 0x200);
            let units: Vec<u16> = record[33..33 + 522]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .take_while(|&unit| unit != 0)
                .collect();
            assert_eq!(record[33 + 522 - 2..33 + 522], [0, 0], "path is NUL padded");
            SdTrack {
                path: String::from_utf16(&units).unwrap(),
                start: be24(record, 6) * 256,
                stop: be24(record, 15) * 256,
                volume: i64::from(be24(record, 24)),
                file_type: be24(record, 27),
                plays_when_shuffling: record[555] != 0,
                bookmarkable: record[556] != 0,
                ..SdTrack::default()
            }
        })
        .collect();
    Sd {
        tracks,
        ..Sd::default()
    }
}

/// The 3G and 4G layout: a `bdhs` header, a `hths` list of `rths` tracks, a `hphs` list of `lphs`
/// playlists, integers little endian, every offset absolute.
pub fn parse_indexed(bytes: &[u8]) -> Sd {
    assert_eq!(&bytes[..4], b"bdhs");
    assert_eq!(le32(bytes, 4), 0x0200_0003);
    assert_eq!(le32(bytes, 8), 64, "bdhs header length");
    let track_count = le32(bytes, 12) as usize;
    let playlist_count = le32(bytes, 16) as usize;
    let hths = le32(bytes, 36) as usize;
    let hphs = le32(bytes, 40) as usize;

    assert_eq!(&bytes[hths..hths + 4], b"hths");
    assert_eq!(le32(bytes, hths + 4) as usize, 20 + 4 * track_count);
    assert_eq!(le32(bytes, hths + 8) as usize, track_count);
    let tracks = (0..track_count)
        .map(|index| {
            let at = le32(bytes, hths + 20 + 4 * index) as usize;
            assert_eq!(&bytes[at..at + 4], b"rths");
            assert_eq!(le32(bytes, at + 4), 372, "rths length");
            let path = &bytes[at + 24..at + 24 + 256];
            let end = path.iter().position(|&byte| byte == 0).unwrap_or(256);
            SdTrack {
                start: le32(bytes, at + 8),
                stop: le32(bytes, at + 12),
                volume: i64::from(le32(bytes, at + 16) as i32),
                file_type: le32(bytes, at + 20),
                path: String::from_utf8(path[..end].to_vec()).unwrap(),
                bookmark_ms: le32(bytes, at + 280),
                plays_when_shuffling: bytes[at + 284] != 0,
                bookmarkable: bytes[at + 285] != 0,
                album_id: le32(bytes, at + 312),
                track_number: le16(bytes, at + 316),
                disc_number: le16(bytes, at + 318),
                dbid: le64(bytes, at + 328),
                artist_id: le32(bytes, at + 336),
            }
        })
        .collect();

    assert_eq!(&bytes[hphs..hphs + 4], b"hphs");
    assert_eq!(le32(bytes, hphs + 4) as usize, 20 + 4 * playlist_count);
    assert_eq!(le16(bytes, hphs + 8) as usize, playlist_count);
    let playlists = (0..playlist_count)
        .map(|index| {
            let at = le32(bytes, hphs + 20 + 4 * index) as usize;
            assert_eq!(&bytes[at..at + 4], b"lphs");
            let length = le32(bytes, at + 4) as usize;
            let count = le32(bytes, at + 8) as usize;
            assert_eq!(length, 44 + 4 * count, "lphs length");
            SdPlaylist {
                non_spoken_count: le32(bytes, at + 12),
                id: le64(bytes, at + 16),
                kind: le32(bytes, at + 24),
                members: (0..count).map(|n| le32(bytes, at + 44 + 4 * n)).collect(),
            }
        })
        .collect();
    Sd {
        tracks,
        playlists,
        volume_limit: bytes[28],
        voiceover: bytes[29] != 0,
        non_spoken_tracks: le32(bytes, 32),
    }
}

/// How the `hphs` header counts its playlists: all, non-podcast, master, non-audiobook.
pub fn playlist_counts(bytes: &[u8]) -> [u16; 4] {
    let hphs = le32(bytes, 40) as usize;
    [
        le16(bytes, hphs + 8),
        le16(bytes, hphs + 12),
        le16(bytes, hphs + 14),
        le16(bytes, hphs + 16),
    ]
}
