//! `iPod_Control/iTunes/iTunesSD`, the track list the iPod shuffle itself reads (it ignores the
//! iTunesDB). Layouts follow libgpod's `itdb_shuffle_write_file`, `write_bdhs`, `write_hths`,
//! `write_rths`, `write_hphs` and `write_lphs` in itdb_itunesdb.c, which in turn follow the
//! iPodLinux and gnupod documentation of the format.

use std::collections::HashMap;

use super::emit::Ids;
use super::mhit::at;
use super::mhod;
use super::raw::{RawDb, RawPlaylist, RawTrack};
use super::view::TrackView;
use crate::bytes::{put, u8_at, u32_at, u64_at};
use crate::error::Result;
use crate::itunesdb::mhod::u32_from;

pub(crate) const FILE_NAME: &str = "iTunesSD";

/// Which iTunesSD the iPod shuffle wants (libgpod's `ItdbShadowDBVersion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShuffleFormat {
    /// Shuffle 1G and 2G: a header, then one fixed 558-byte, big-endian record per track.
    Fixed,
    /// Shuffle 3G and 4G: `bdhs`, `hths` and `hphs` chunks with playlists, little-endian.
    Indexed,
}

pub(crate) fn emit(db: &RawDb, format: ShuffleFormat, volume_limit: u8) -> Result<Vec<u8>> {
    let views = db
        .tracks
        .iter()
        .map(TrackView::of)
        .collect::<Result<Vec<_>>>()?;
    let ids = Ids::assign(&views);
    let tracks = db
        .tracks
        .iter()
        .zip(ids.album.iter().zip(&ids.artist))
        .map(|(track, (&album_id, &artist_id))| Entry::of(track, album_id, artist_id))
        .collect::<Result<Vec<_>>>()?;
    Ok(match format {
        ShuffleFormat::Fixed => fixed(&tracks),
        ShuffleFormat::Indexed => indexed(&tracks, db, volume_limit),
    })
}

/// Where the volume limit set in iTunes sits in a 3G/4G iTunesSD (`bdhs` offset 28), carried over
/// so a sync does not lift a limit someone set for their ears. VoiceOver is always written off: it
/// needs spoken-name files this crate does not make.
const BDHS_VOLUME_LIMIT_AT: usize = 28;

/// The volume limit stored in an existing 3G/4G iTunesSD, 0 (none) if it is not one.
pub(crate) fn volume_limit(existing: &[u8]) -> u8 {
    if existing.get(..4) == Some(b"bdhs") {
        u8_at(existing, BDHS_VOLUME_LIMIT_AT).unwrap_or(0)
    } else {
        0
    }
}

/// Everything one track record needs, read from the track's header and mhods.
struct Entry {
    path: String,
    start_ms: u32,
    stop_ms: u32,
    volume: i32,
    file_type: u32,
    bookmark_ms: u32,
    skip_when_shuffling: bool,
    remember_position: bool,
    pregap: u32,
    postgap: u32,
    sample_count: u32,
    gapless_data: u32,
    album_id: u32,
    artist_id: u32,
    track_number: u16,
    disc_number: u16,
    dbid: u64,
    podcast: bool,
    audiobook: bool,
}

const MEDIA_PODCAST: u32 = 1 << 2;
const MEDIA_AUDIOBOOK: u32 = 1 << 3;

impl Entry {
    /// Podcasts and audiobooks are counted apart from music in the iTunesSD.
    fn spoken_word(&self) -> bool {
        self.podcast || self.audiobook
    }

    fn of(track: &RawTrack, album_id: u32, artist_id: u32) -> Result<Self> {
        let header = &track.header;
        let number = |at| u32_at(header, at).unwrap_or(0);
        let media_type = number(at::MEDIA_TYPE);
        // The shuffle separates folders with forward slashes, not the iTunesDB's colons.
        let path = track
            .text(mhod::PATH)?
            .unwrap_or_default()
            .replace(':', "/");
        let file_type = track.text(mhod::FILE_TYPE)?.unwrap_or_default();
        Ok(Self {
            path,
            start_ms: number(at::START_TIME),
            stop_ms: number(at::STOP_TIME),
            volume: i32::from_le_bytes(number(at::VOLUME).to_le_bytes()),
            file_type: shuffle_file_type(&file_type),
            bookmark_ms: number(at::BOOKMARK_TIME),
            skip_when_shuffling: u8_at(header, at::SKIP_WHEN_SHUFFLING) == Some(1),
            remember_position: u8_at(header, at::REMEMBER_POSITION).is_some_and(|flag| flag != 0),
            pregap: number(at::PREGAP),
            postgap: number(at::POSTGAP),
            // libgpod keeps only the low 32 bits of the 64-bit sample count.
            sample_count: u64_at(header, at::SAMPLE_COUNT).map_or(0, |count| count as u32),
            gapless_data: number(at::GAPLESS_DATA),
            album_id,
            artist_id,
            track_number: u16::try_from(number(at::TRACK_NUMBER)).unwrap_or(0),
            disc_number: u16::try_from(number(at::DISC_NUMBER)).unwrap_or(0),
            dbid: track.dbid(),
            podcast: media_type & MEDIA_PODCAST != 0,
            audiobook: media_type & MEDIA_AUDIOBOOK != 0,
        })
    }
}

/// libgpod's `convert_filetype`: 1 is MP3, 2 AAC, 4 WAV, judged from the iTunesDB's type string.
fn shuffle_file_type(type_name: &str) -> u32 {
    const AAC: [&str; 6] = ["AAC", "MP4", "M4A", "aac", "mp4", "m4a"];
    const WAV: [&str; 2] = ["WAV", "wav"];
    if AAC.iter().any(|name| type_name.contains(name)) {
        2
    } else if WAV.iter().any(|name| type_name.contains(name)) {
        4
    } else {
        1
    }
}

fn push_u24_be(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.min(0x00ff_ffff).to_be_bytes()[1..]);
}

const FIXED_HEADER_LEN: u32 = 0x12;
const FIXED_RECORD_LEN: u32 = 0x22e;
const FIXED_PATH_UNITS: usize = 261;

/// libgpod's old-format branch of `itdb_shuffle_write_file`.
fn fixed(tracks: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    push_u24_be(&mut out, u32_from(tracks.len()));
    push_u24_be(&mut out, 0x01_0600);
    push_u24_be(&mut out, FIXED_HEADER_LEN);
    for _ in 0..3 {
        push_u24_be(&mut out, 0);
    }
    for track in tracks {
        push_u24_be(&mut out, FIXED_RECORD_LEN);
        push_u24_be(&mut out, 0x5a_a501);
        // Start and stop are in 256 ms steps.
        push_u24_be(&mut out, track.start_ms / 256);
        push_u24_be(&mut out, 0);
        push_u24_be(&mut out, 0);
        push_u24_be(&mut out, track.stop_ms / 256);
        push_u24_be(&mut out, 0);
        push_u24_be(&mut out, 0);
        // The iTunesDB's -255..255 becomes 0..200, 100 being no adjustment.
        let volume = (track.volume.clamp(-255, 255) + 255) * 201 / 511;
        push_u24_be(&mut out, u32::try_from(volume).unwrap_or(100));
        push_u24_be(&mut out, track.file_type);
        push_u24_be(&mut out, 0x200);
        let mut units: Vec<u16> = track.path.encode_utf16().take(FIXED_PATH_UNITS).collect();
        units.resize(FIXED_PATH_UNITS, 0);
        out.extend(units.into_iter().flat_map(u16::to_le_bytes));
        // 0 skips the track in shuffle mode, the opposite of the iTunesDB's flag. libgpod copies the
        // flag unchanged here, which would skip every ordinary track; its 3G/4G writer and the
        // format documentation it cites both invert it, so this follows them.
        out.push(u8::from(!track.skip_when_shuffling));
        out.push(u8::from(track.remember_position));
        out.push(0);
    }
    out
}

const BDHS_LEN: usize = 64;
const RTHS_PATH_LEN: usize = 256;

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn zeros(out: &mut Vec<u8>, count: usize) {
    out.extend(std::iter::repeat_n(0u8, count));
}

/// A playlist as the indexed iTunesSD lists it: its type and its members' places in the track list.
struct Listed<'a> {
    playlist: &'a RawPlaylist,
    kind: Kind,
    members: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Master = 1,
    Ordinary = 2,
    Podcasts = 3,
    Audiobooks = 4,
}

impl<'a> Listed<'a> {
    /// `None` for a playlist with no members, which the iTunesSD leaves out.
    fn of(
        playlist: &'a RawPlaylist,
        tracks: &[Entry],
        position_of: &HashMap<u32, usize>,
    ) -> Option<Self> {
        let members: Vec<usize> = playlist
            .members
            .iter()
            .filter_map(|id| position_of.get(id).copied())
            .collect();
        if members.is_empty() {
            return None;
        }
        // libgpod's order of precedence; an audiobook playlist is one made only of audiobooks.
        let kind = if playlist.is_master() {
            Kind::Master
        } else if playlist.is_podcasts() {
            Kind::Podcasts
        } else if members.iter().all(|&position| tracks[position].audiobook) {
            Kind::Audiobooks
        } else {
            Kind::Ordinary
        };
        Some(Self {
            playlist,
            kind,
            members,
        })
    }
}

/// libgpod's new-format branch: `write_bdhs`, then `write_hths` and `write_hphs`.
fn indexed(tracks: &[Entry], db: &RawDb, volume_limit: u8) -> Vec<u8> {
    let position_of: HashMap<u32, usize> = db
        .tracks
        .iter()
        .enumerate()
        .map(|(position, track)| (track.id(), position))
        .collect();
    let playlists: Vec<Listed> = db
        .playlists
        .iter()
        .filter_map(|playlist| Listed::of(playlist, tracks, &position_of))
        .collect();
    let ordinary = tracks.iter().filter(|track| !track.spoken_word()).count();

    let mut out = vec![0u8; BDHS_LEN];
    put(&mut out, 0, b"bdhs");
    put(&mut out, 4, &0x0200_0003u32.to_le_bytes());
    put(&mut out, 8, &u32_from(BDHS_LEN).to_le_bytes());
    put(&mut out, 12, &u32_from(tracks.len()).to_le_bytes());
    put(&mut out, 16, &u32_from(playlists.len()).to_le_bytes());
    // The byte after the volume limit is the VoiceOver switch, left off.
    put(&mut out, BDHS_VOLUME_LIMIT_AT, &[volume_limit, 0]);
    put(&mut out, 32, &u32_from(ordinary).to_le_bytes());

    // Track list: header, one offset per track, then the records.
    let hths = u32_from(out.len());
    put(&mut out, 36, &hths.to_le_bytes());
    out.extend_from_slice(b"hths");
    push_u32(&mut out, u32_from(20 + 4 * tracks.len()));
    push_u32(&mut out, u32_from(tracks.len()));
    zeros(&mut out, 8);
    let offsets = out.len();
    zeros(&mut out, 4 * tracks.len());
    for (index, track) in tracks.iter().enumerate() {
        let here = u32_from(out.len());
        put(&mut out, offsets + 4 * index, &here.to_le_bytes());
        write_rths(&mut out, track);
    }

    // Playlist list: header, one offset per playlist, then the playlists.
    let hphs = u32_from(out.len());
    put(&mut out, 40, &hphs.to_le_bytes());
    let of_kind = |kind| {
        u16::try_from(
            playlists
                .iter()
                .filter(|listed| listed.kind == kind)
                .count(),
        )
        .unwrap_or(0)
    };
    let count = u16::try_from(playlists.len()).unwrap_or(u16::MAX);
    let counts = [
        count,
        0,
        count - of_kind(Kind::Podcasts),
        of_kind(Kind::Master),
        count - of_kind(Kind::Audiobooks),
        0,
    ];
    out.extend_from_slice(b"hphs");
    push_u32(&mut out, u32_from(20 + 4 * playlists.len()));
    out.extend(counts.into_iter().flat_map(u16::to_le_bytes));
    let offsets = out.len();
    zeros(&mut out, 4 * playlists.len());
    for (index, listed) in playlists.iter().enumerate() {
        let here = u32_from(out.len());
        put(&mut out, offsets + 4 * index, &here.to_le_bytes());
        write_lphs(&mut out, listed, tracks);
    }
    out
}

/// One track in the indexed iTunesSD: `rths`, 372 bytes.
fn write_rths(out: &mut Vec<u8>, track: &Entry) {
    let start = out.len();
    out.extend_from_slice(b"rths");
    push_u32(out, 0);
    push_u32(out, track.start_ms);
    push_u32(out, track.stop_ms);
    push_u32(out, u32::from_le_bytes(track.volume.to_le_bytes()));
    push_u32(out, track.file_type);
    let mut path = track.path.as_bytes().to_vec();
    path.resize(RTHS_PATH_LEN, 0);
    out.extend_from_slice(&path);
    push_u32(out, track.bookmark_ms);
    // As in the 1G/2G records, this flag is 1 where the iTunesDB's is 0.
    out.extend_from_slice(&[
        u8::from(!track.skip_when_shuffling),
        u8::from(track.remember_position),
        0,
        0,
    ]);
    push_u32(out, track.pregap);
    push_u32(out, track.postgap);
    push_u32(out, track.sample_count);
    zeros(out, 4);
    push_u32(out, track.gapless_data);
    zeros(out, 4);
    push_u32(out, track.album_id);
    out.extend_from_slice(&track.track_number.to_le_bytes());
    out.extend_from_slice(&track.disc_number.to_le_bytes());
    zeros(out, 8);
    out.extend_from_slice(&track.dbid.to_le_bytes());
    push_u32(out, track.artist_id);
    zeros(out, 32);
    let length = u32_from(out.len() - start);
    put(out, start + 4, &length.to_le_bytes());
}

/// One playlist in the indexed iTunesSD: `lphs`, its type, then the position of each member in the
/// track list.
fn write_lphs(out: &mut Vec<u8>, listed: &Listed, tracks: &[Entry]) {
    let start = out.len();
    let spoken = listed
        .members
        .iter()
        .filter(|&&position| tracks[position].spoken_word())
        .count();
    out.extend_from_slice(b"lphs");
    push_u32(out, 0);
    push_u32(out, u32_from(listed.members.len()));
    push_u32(out, u32_from(listed.members.len() - spoken));
    if listed.kind == Kind::Master {
        zeros(out, 8);
    } else {
        out.extend_from_slice(&listed.playlist.id().to_le_bytes());
    }
    push_u32(out, listed.kind as u32);
    zeros(out, 16);
    for &position in &listed.members {
        push_u32(out, u32_from(position));
    }
    let length = u32_from(out.len() - start);
    put(out, start + 4, &length.to_le_bytes());
}
