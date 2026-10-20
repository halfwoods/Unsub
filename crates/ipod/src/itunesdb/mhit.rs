//! mhlt/mhit: the track list. Field offsets follow libgpod's `get_mhit` (itdb_itunesdb.c), which
//! documents which iTunes version added each field; a header too short to hold one leaves it unset.

use std::path::{Component, Path, PathBuf};

use super::chunks::{Chunk, ListHead, MHIT, MHLT};
use super::mhod::{self, Mhod};
use super::raw::{RawMhod, RawTrack};
use crate::database::{IpodMediaKind, IpodTrack};
use crate::error::Result;
use crate::time::mac_to_unix;

/// Header offsets, from libgpod's `get_mhit` (reading) and `mk_mhit` (writing).
pub(super) mod at {
    pub(in crate::itunesdb) const MHOD_COUNT: usize = 12;
    pub(in crate::itunesdb) const ID: usize = 16;
    pub(in crate::itunesdb) const VISIBLE: usize = 20;
    pub(in crate::itunesdb) const FILE_TYPE_MARKER: usize = 24;
    pub(in crate::itunesdb) const TYPE2: usize = 29;
    pub(in crate::itunesdb) const COMPILATION: usize = 30;
    pub(in crate::itunesdb) const RATING: usize = 31;
    pub(in crate::itunesdb) const TIME_MODIFIED: usize = 32;
    pub(in crate::itunesdb) const FILE_SIZE: usize = 36;
    pub(in crate::itunesdb) const DURATION_MS: usize = 40;
    pub(in crate::itunesdb) const TRACK_NUMBER: usize = 44;
    pub(in crate::itunesdb) const TRACK_COUNT: usize = 48;
    pub(in crate::itunesdb) const YEAR: usize = 52;
    pub(in crate::itunesdb) const BITRATE: usize = 56;
    pub(in crate::itunesdb) const SAMPLE_RATE: usize = 60;
    pub(in crate::itunesdb) const PLAY_COUNT: usize = 80;
    pub(in crate::itunesdb) const PLAY_COUNT_REPEAT: usize = 84;
    pub(in crate::itunesdb) const LAST_PLAYED: usize = 88;
    pub(in crate::itunesdb) const DISC_NUMBER: usize = 92;
    pub(in crate::itunesdb) const DISC_COUNT: usize = 96;
    pub(in crate::itunesdb) const DATE_ADDED: usize = 104;
    pub(in crate::itunesdb) const DBID: usize = 112;
    pub(in crate::itunesdb) const APP_RATING: usize = 121;
    pub(in crate::itunesdb) const UNKNOWN_126: usize = 126;
    pub(in crate::itunesdb) const SAMPLE_RATE_FLOAT: usize = 136;
    pub(in crate::itunesdb) const UNKNOWN_144: usize = 144;
    pub(in crate::itunesdb) const SKIP_COUNT: usize = 156;
    pub(in crate::itunesdb) const HAS_ARTWORK: usize = 164;
    pub(in crate::itunesdb) const SKIP_WHEN_SHUFFLING: usize = 165;
    pub(in crate::itunesdb) const REMEMBER_POSITION: usize = 166;
    pub(in crate::itunesdb) const DBID_REPEAT: usize = 168;
    pub(in crate::itunesdb) const MOVIE_FLAG: usize = 177;
    pub(in crate::itunesdb) const MARK_UNPLAYED: usize = 178;
    pub(in crate::itunesdb) const MEDIA_TYPE: usize = 208;
    pub(in crate::itunesdb) const ALBUM_ID: usize = 0x120;
    /// Repeats the mhbd's field at 0x24; purpose unknown, but libgpod writes it.
    pub(in crate::itunesdb) const DATABASE_TAG: usize = 0x124;
    pub(in crate::itunesdb) const FILE_SIZE_REPEAT: usize = 0x12c;
    /// libgpod: "what the heck is this?!" iTunes writes 0x808080808080 here.
    pub(in crate::itunesdb) const UNKNOWN_0X134: usize = 0x134;
    pub(in crate::itunesdb) const ARTWORK_LINK_FLAG: usize = 0x168;
    pub(in crate::itunesdb) const ARTWORK_COUNT: usize = 124;
    pub(in crate::itunesdb) const ARTWORK_SIZE: usize = 128;
    pub(in crate::itunesdb) const MHII_LINK: usize = 0x160;
    pub(in crate::itunesdb) const ARTIST_ID: usize = 0x1e0;
    pub(in crate::itunesdb) const COMPOSER_ID: usize = 0x1f4;
}

const MEDIA_PODCAST: u32 = 1 << 2;
const MEDIA_AUDIOBOOK: u32 = 1 << 3;
const MEDIA_VIDEO: u32 = (1 << 1) | (1 << 5) | (1 << 6);
const MEDIA_AUDIO: u32 = 1 << 0;

pub(super) fn parse_list(data: &[u8], section: &Chunk, mount: &Path) -> Result<Vec<IpodTrack>> {
    let list = ListHead::find(data, section, MHLT)?;
    let mut tracks = Vec::new();
    let mut offset = list.items_offset;
    while tracks.len() < list.count as usize && offset < section.end {
        let chunk = Chunk::read(data, offset, section.end)?;
        offset = chunk.end;
        if chunk.tag == MHIT {
            tracks.push(parse_track(data, &chunk, mount)?);
        }
    }
    Ok(tracks)
}

/// The same list as [`parse_list`], kept as bytes so a rewrite can reproduce it exactly.
pub(super) fn parse_raw_list(
    data: &[u8],
    section: &Chunk,
    list: &ListHead,
) -> Result<Vec<RawTrack>> {
    let mut tracks = Vec::new();
    let mut offset = list.items_offset;
    while tracks.len() < list.count as usize && offset < section.end {
        let chunk = Chunk::read(data, offset, section.end)?;
        offset = chunk.end;
        if chunk.tag != MHIT {
            continue;
        }
        let mut mhods = Vec::new();
        let mut mhod_offset = chunk.header_end();
        for _ in 0..chunk.u32(at::MHOD_COUNT).unwrap_or(0) {
            if mhod_offset >= chunk.end {
                break;
            }
            let mhod = Mhod::read(data, mhod_offset, chunk.end)?;
            let bytes = data
                .get(mhod_offset..mhod.end())
                .unwrap_or_default()
                .to_vec();
            mhods.push(RawMhod {
                kind: mhod.kind,
                bytes,
            });
            mhod_offset = mhod.end();
        }
        tracks.push(RawTrack {
            header: chunk.header().to_vec(),
            mhods,
        });
    }
    Ok(tracks)
}

fn parse_track(data: &[u8], chunk: &Chunk, mount: &Path) -> Result<IpodTrack> {
    let nonzero = |field| chunk.u32(field).filter(|&value| value != 0);
    let mut track = IpodTrack {
        id: chunk.u32(at::ID).unwrap_or(0),
        dbid: chunk.u64(at::DBID).unwrap_or(0),
        year: nonzero(at::YEAR).and_then(|year| i32::try_from(year).ok()),
        track_number: nonzero(at::TRACK_NUMBER),
        track_count: nonzero(at::TRACK_COUNT),
        disc_number: nonzero(at::DISC_NUMBER),
        disc_count: nonzero(at::DISC_COUNT),
        duration_ms: u64::from(chunk.u32(at::DURATION_MS).unwrap_or(0)),
        bitrate_kbps: nonzero(at::BITRATE),
        // Stored as Hz << 16; the low half is unused fraction.
        sample_rate: nonzero(at::SAMPLE_RATE)
            .map(|raw| raw >> 16)
            .filter(|&hz| hz != 0),
        file_size: u64::from(chunk.u32(at::FILE_SIZE).unwrap_or(0)),
        play_count: chunk.u32(at::PLAY_COUNT).unwrap_or(0),
        last_played: chunk.u32(at::LAST_PLAYED).and_then(mac_to_unix),
        skip_count: chunk.u32(at::SKIP_COUNT).unwrap_or(0),
        rating: chunk.u8(at::RATING).unwrap_or(0).min(100),
        date_added: chunk.u32(at::DATE_ADDED).and_then(mac_to_unix),
        media_kind: media_kind(
            chunk.u32(at::MEDIA_TYPE),
            chunk.u8(at::MOVIE_FLAG) == Some(1),
        ),
        compilation: chunk.u8(at::COMPILATION).is_some_and(|flag| flag != 0),
        ..IpodTrack::default()
    };

    let mut offset = chunk.header_end();
    for _ in 0..chunk.u32(at::MHOD_COUNT).unwrap_or(0) {
        if offset >= chunk.end {
            break;
        }
        let mhod = Mhod::read(data, offset, chunk.end)?;
        offset = mhod.end();
        match mhod.kind {
            mhod::TITLE => track.title = mhod.text()?,
            mhod::PATH => track.path = native_path(mount, &mhod.text()?),
            mhod::ALBUM => track.album = mhod.text()?,
            mhod::ARTIST => track.artist = mhod.text()?,
            mhod::GENRE => track.genre = mhod.text()?,
            mhod::FILE_TYPE => track.file_type = mhod.text()?,
            mhod::COMPOSER => track.composer = mhod.text()?,
            mhod::ALBUM_ARTIST => track.album_artist = mhod.text()?,
            mhod::SORT_TITLE => track.sort_title = sort_key(&mhod)?,
            mhod::SORT_ARTIST => track.sort_artist = sort_key(&mhod)?,
            mhod::SORT_ALBUM => track.sort_album = sort_key(&mhod)?,
            mhod::SORT_ALBUM_ARTIST => track.sort_album_artist = sort_key(&mhod)?,
            _ => {}
        }
    }

    if track.file_type.is_empty() {
        track.file_type = chunk
            .u32(at::FILE_TYPE_MARKER)
            .map(four_cc)
            .unwrap_or_default();
    }
    Ok(track)
}

fn sort_key(mhod: &Mhod) -> Result<Option<String>> {
    Ok(Some(mhod.text()?).filter(|text| !text.is_empty()))
}

/// Hard-drive iPods leave the marker 0; flash iPods store the extension as ASCII, e.g. `MP3 `.
fn four_cc(marker: u32) -> String {
    let bytes = marker.to_be_bytes();
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ' || *byte == 0)
    {
        return String::new();
    }
    String::from_utf8_lossy(&bytes)
        .trim_matches([' ', '\0'])
        .to_owned()
}

/// libgpod's `Itdb_Mediatype`. A zero type shows up in both audio and video menus on the iPod, so
/// the movie flag breaks the tie.
fn media_kind(media_type: Option<u32>, movie_flag: bool) -> IpodMediaKind {
    match media_type {
        None | Some(0) if movie_flag => IpodMediaKind::Video,
        None | Some(0) => IpodMediaKind::Music,
        Some(kind) if kind & MEDIA_PODCAST != 0 => IpodMediaKind::Podcast,
        Some(kind) if kind & MEDIA_AUDIOBOOK != 0 => IpodMediaKind::Audiobook,
        Some(kind) if kind & MEDIA_VIDEO != 0 => IpodMediaKind::Video,
        Some(kind) if kind & MEDIA_AUDIO != 0 => IpodMediaKind::Music,
        Some(_) => IpodMediaKind::Other,
    }
}

/// `:iPod_Control:Music:F00:ABCD.mp3` to an absolute path under `mount`. The database is untrusted
/// input, so any component that is not a plain name (`..`, a drive prefix) is dropped rather than
/// allowed to leave the mount.
pub(crate) fn native_path(mount: &Path, ipod_path: &str) -> PathBuf {
    ipod_path
        .split(':')
        .filter(|part| {
            let mut components = Path::new(part).components();
            matches!(
                (components.next(), components.next()),
                (Some(Component::Normal(_)), None)
            )
        })
        .fold(mount.to_path_buf(), |path, part| path.join(part))
}
