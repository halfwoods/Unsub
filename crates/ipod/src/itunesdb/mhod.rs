//! mhod: one typed value, usually a string, attached to a track, playlist or playlist entry. Type
//! numbers follow libgpod's `enum MHOD_ID` (itdb_itunesdb.c); `get_mhod` shows the payload layouts.

use super::chunks::{Chunk, MHOD, tag_name};
use crate::bytes::u32_at;
use crate::error::{Error, Result};

pub(super) const TITLE: u32 = 1;
pub(super) const PATH: u32 = 2;
pub(super) const ALBUM: u32 = 3;
pub(super) const ARTIST: u32 = 4;
pub(super) const GENRE: u32 = 5;
pub(super) const FILE_TYPE: u32 = 6;
pub(super) const COMPOSER: u32 = 12;
pub(super) const ALBUM_ARTIST: u32 = 22;
pub(super) const SORT_ARTIST: u32 = 23;
pub(super) const SORT_TITLE: u32 = 27;
pub(super) const SORT_ALBUM: u32 = 28;
pub(super) const SORT_ALBUM_ARTIST: u32 = 29;
pub(super) const SMART_PREFS: u32 = 50;
pub(super) const SMART_RULES: u32 = 51;
pub(super) const POSITION: u32 = 100;

const TYPE_AT: usize = 12;
/// The string payload follows the encoding flag, the byte length and 8 unknown bytes.
const TEXT_DATA_AT: usize = 16;
/// Encoding flag 2 is UTF-8; 0 and 1 are UTF-16LE.
const UTF8_FLAG: u32 = 2;

pub(super) struct Mhod<'a> {
    pub(super) kind: u32,
    chunk: Chunk<'a>,
}

impl<'a> Mhod<'a> {
    pub(super) fn read(data: &'a [u8], offset: usize, limit: usize) -> Result<Self> {
        let chunk = Chunk::read(data, offset, limit)?;
        if chunk.tag != MHOD {
            return Err(Error::corrupt(
                offset,
                format!("expected mhod, found {}", tag_name(chunk.tag)),
            ));
        }
        let kind = chunk
            .u32(TYPE_AT)
            .ok_or_else(|| Error::corrupt(offset, "mhod header has no type field"))?;
        Ok(Self { kind, chunk })
    }

    pub(super) fn end(&self) -> usize {
        self.chunk.end
    }

    pub(super) fn text(&self) -> Result<String> {
        let body = self.chunk.body;
        let (Some(encoding), Some(len)) = (u32_at(body, 0), u32_at(body, 4)) else {
            return Err(Error::corrupt(
                self.chunk.offset,
                "string mhod is too short",
            ));
        };
        let bytes = body
            .get(TEXT_DATA_AT..)
            .and_then(|rest| rest.get(..len as usize))
            .ok_or_else(|| {
                Error::corrupt(self.chunk.offset, "string runs past the end of its mhod")
            })?;
        let text = if encoding == UTF8_FLAG {
            String::from_utf8_lossy(bytes).into_owned()
        } else {
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        };
        Ok(text.trim_end_matches('\0').to_owned())
    }

    /// The playlist position carried by a `POSITION` mhod.
    pub(super) fn position(&self) -> Option<u32> {
        u32_at(self.chunk.body, 0)
    }
}
