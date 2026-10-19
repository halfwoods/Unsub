//! Reader for iTunesDB and iTunesCDB. Structure and offsets follow libgpod's `parse_fimp`,
//! `find_mhsd` and `itdb_zlib_check_decompress_fimp`; see the sibling modules for each record type.

mod chunks;
mod edit;
mod emit;
mod hash58;
mod index;
mod mhit;
mod mhod;
mod playlists;
pub(crate) mod raw;
mod view;

use std::fs;
use std::io::Read;
use std::path::Path;

use flate2::read::ZlibDecoder;

use self::chunks::{Chunk, MHBD, MHSD};
pub(crate) use self::edit::{Codec, Metadata, Origin};
pub(crate) use self::emit::emit;
pub(crate) use self::mhit::native_path;
pub(crate) use self::mhod::PATH;
use crate::bytes::{tag_at, u32_at};
use crate::database::{IpodPlaylist, IpodTrack};
use crate::device::DatabaseFormat;
use crate::error::{Error, Result};

const MHSD_TYPE_AT: usize = 12;
const MHBD_SECTION_COUNT_AT: usize = 20;
const SECTION_TRACKS: u32 = 1;
const SECTION_PLAYLISTS: u32 = 2;
const SECTION_PODCAST_PLAYLISTS: u32 = 3;

/// iTunesCDB keeps the mhbd header plain and deflates the rest; libgpod requires the header to
/// reach the compression flag at 0xa8.
const CDB_MIN_HEADER_LEN: usize = 0xa9;
/// A corrupt zlib stream must not be able to exhaust memory; real databases are tens of megabytes.
const MAX_INFLATED_LEN: u64 = 1 << 30;

pub(crate) fn load(path: &Path, format: DatabaseFormat) -> Result<Vec<u8>> {
    let bytes = fs::read(path).map_err(|source| Error::io(path, source))?;
    match format {
        DatabaseFormat::ITunesDb => Ok(bytes),
        DatabaseFormat::ITunesCdb => inflate_cdb(&bytes),
    }
}

fn inflate_cdb(file: &[u8]) -> Result<Vec<u8>> {
    let header_len = u32_at(file, 4).map_or(0, |len| len as usize);
    if tag_at(file, 0) != Some(MHBD) || header_len < CDB_MIN_HEADER_LEN || header_len > file.len() {
        return Err(Error::corrupt(0, "not an iTunesCDB: bad mhbd header"));
    }
    let (header, compressed) = file.split_at(header_len);
    let mut database = header.to_vec();
    ZlibDecoder::new(compressed)
        .take(MAX_INFLATED_LEN + 1)
        .read_to_end(&mut database)
        .map_err(|error| {
            Error::corrupt(
                header_len,
                format!("iTunesCDB body is not valid zlib data: {error}"),
            )
        })?;
    if database.len() as u64 > header_len as u64 + MAX_INFLATED_LEN {
        return Err(Error::corrupt(
            header_len,
            "iTunesCDB body inflates to an implausible size",
        ));
    }
    // The stored total length is the compressed size; make it describe the buffer we parse.
    let total_len =
        u32::try_from(database.len()).map_err(|_| Error::corrupt(0, "iTunesCDB is too large"))?;
    database[8..12].copy_from_slice(&total_len.to_le_bytes());
    Ok(database)
}

pub(crate) fn parse(data: &[u8], mount: &Path) -> Result<(Vec<IpodTrack>, Vec<IpodPlaylist>)> {
    let sections = Sections::locate(data)?;
    let tracks = mhit::parse_list(data, &sections.tracks, mount)?;
    let playlists = playlists::parse_list(data, &sections.playlists)?;
    Ok((tracks, playlists))
}

/// The name iTunes showed for the device: the title of the master playlist.
pub(crate) fn master_playlist_name(data: &[u8]) -> Result<Option<String>> {
    let sections = Sections::locate(data)?;
    let playlists = playlists::parse_list(data, &sections.playlists)?;
    Ok(playlists
        .into_iter()
        .find(|playlist| playlist.is_master)
        .map(|playlist| playlist.name))
}

struct Sections<'a> {
    tracks: Chunk<'a>,
    playlists: Chunk<'a>,
}

impl<'a> Sections<'a> {
    /// Types 4 and up (albums, artists, smart playlists, ...) are not needed and are skipped.
    fn locate(data: &'a [u8]) -> Result<Self> {
        let database = database_chunk(data)?;
        let (mut tracks, mut playlists, mut podcast_playlists) = (None, None, None);
        for section in sections(data, &database)? {
            match section.u32(MHSD_TYPE_AT) {
                Some(SECTION_TRACKS) => tracks = tracks.or(Some(section)),
                Some(SECTION_PLAYLISTS) => playlists = playlists.or(Some(section)),
                Some(SECTION_PODCAST_PLAYLISTS) => {
                    podcast_playlists = podcast_playlists.or(Some(section));
                }
                _ => {}
            }
        }

        Ok(Self {
            tracks: tracks
                .ok_or_else(|| Error::corrupt(0, "no track list section (mhsd type 1)"))?,
            // libgpod prefers type 3 over type 2 when both exist.
            playlists: podcast_playlists
                .or(playlists)
                .ok_or_else(|| Error::corrupt(0, "no playlist section (mhsd type 2 or 3)"))?,
        })
    }
}

fn database_chunk(data: &[u8]) -> Result<Chunk<'_>> {
    let database = Chunk::read(data, 0, data.len())?;
    if database.tag != MHBD {
        let reason = if database.tag == *b"dbhm" {
            "big-endian databases (mobile phones) are not supported"
        } else {
            "not an iTunesDB: missing mhbd header"
        };
        return Err(Error::corrupt(0, reason));
    }
    Ok(database)
}

/// The mhsd children of the mhbd, in file order. Other chunk types are skipped.
fn sections<'a>(data: &'a [u8], database: &Chunk<'a>) -> Result<Vec<Chunk<'a>>> {
    let mut found = Vec::new();
    let mut offset = database.header_end();
    for _ in 0..database.u32(MHBD_SECTION_COUNT_AT).unwrap_or(0) {
        if offset >= database.end {
            break;
        }
        let section = Chunk::read(data, offset, database.end)?;
        offset = section.end;
        if section.tag == MHSD {
            found.push(section);
        }
    }
    Ok(found)
}
