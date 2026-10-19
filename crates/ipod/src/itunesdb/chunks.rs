//! Bounds-checked access to the chunk framing shared by every iTunesDB record: a four-byte tag,
//! then `header_len` and `total_len` as little-endian `u32`. Nothing here indexes a slice directly.

use crate::bytes::{tag_at, u8_at, u16_at, u32_at, u64_at};
use crate::error::{Error, Result};

pub(super) const MHBD: [u8; 4] = *b"mhbd";
pub(super) const MHSD: [u8; 4] = *b"mhsd";
pub(super) const MHLT: [u8; 4] = *b"mhlt";
pub(super) const MHIT: [u8; 4] = *b"mhit";
pub(super) const MHLP: [u8; 4] = *b"mhlp";
pub(super) const MHYP: [u8; 4] = *b"mhyp";
pub(super) const MHIP: [u8; 4] = *b"mhip";
pub(super) const MHOD: [u8; 4] = *b"mhod";

/// Tag, header length and total length: the part every chunk has in common.
const PREFIX_LEN: usize = 12;

/// One record whose `total_len` field is a byte length. Field getters are limited to the header, so
/// a record written by an older iTunes with a short header yields `None` instead of reading into
/// its children.
#[derive(Clone, Copy)]
pub(super) struct Chunk<'a> {
    pub(super) offset: usize,
    pub(super) tag: [u8; 4],
    header: &'a [u8],
    /// Everything between the end of the header and `total_len`.
    pub(super) body: &'a [u8],
    pub(super) end: usize,
}

impl<'a> Chunk<'a> {
    /// Reads the chunk at `offset`, which must lie entirely within `data[..limit]`.
    pub(super) fn read(data: &'a [u8], offset: usize, limit: usize) -> Result<Self> {
        let window = window(data, offset, limit)?;
        let (tag, header_len, total_len) = prefix(window, offset)?;
        let (header_len, total_len) = (header_len as usize, total_len as usize);
        if header_len < PREFIX_LEN {
            return Err(Error::corrupt(
                offset,
                format!("header length {header_len} is below {PREFIX_LEN}"),
            ));
        }
        if total_len < header_len {
            return Err(Error::corrupt(
                offset,
                format!("total length {total_len} is below header length {header_len}"),
            ));
        }
        if total_len > window.len() {
            return Err(Error::corrupt(
                offset,
                format!("chunk of {total_len} bytes runs past the end of its parent"),
            ));
        }
        Ok(Self {
            offset,
            tag,
            header: &window[..header_len],
            body: &window[header_len..total_len],
            end: offset + total_len,
        })
    }

    pub(super) fn header_end(&self) -> usize {
        self.offset + self.header.len()
    }

    pub(super) fn u8(&self, at: usize) -> Option<u8> {
        u8_at(self.header, at)
    }

    pub(super) fn u16(&self, at: usize) -> Option<u16> {
        u16_at(self.header, at)
    }

    pub(super) fn u32(&self, at: usize) -> Option<u32> {
        u32_at(self.header, at)
    }

    pub(super) fn u64(&self, at: usize) -> Option<u64> {
        u64_at(self.header, at)
    }
}

/// mhlt and mhlp reuse the `total_len` slot for an item count; their items follow the header as
/// siblings rather than as children.
pub(super) struct ListHead {
    pub(super) count: u32,
    pub(super) items_offset: usize,
}

impl ListHead {
    fn read(data: &[u8], offset: usize, limit: usize, expected: [u8; 4]) -> Result<Self> {
        let window = window(data, offset, limit)?;
        let (tag, header_len, count) = prefix(window, offset)?;
        if tag != expected {
            return Err(Error::corrupt(
                offset,
                format!("expected {}, found {}", tag_name(expected), tag_name(tag)),
            ));
        }
        let header_len = header_len as usize;
        if header_len < PREFIX_LEN || header_len > window.len() {
            return Err(Error::corrupt(
                offset,
                format!("list header length {header_len} is out of range"),
            ));
        }
        Ok(Self {
            count,
            items_offset: offset + header_len,
        })
    }

    /// Finds the list inside an mhsd, skipping chunk types this reader does not know.
    pub(super) fn find(data: &[u8], section: &Chunk, expected: [u8; 4]) -> Result<Self> {
        let mut at = section.header_end();
        while at < section.end {
            if tag_at(data, at) == Some(expected) {
                return Self::read(data, at, section.end, expected);
            }
            at = Chunk::read(data, at, section.end)?.end;
        }
        Err(Error::corrupt(
            section.offset,
            format!("section has no {}", tag_name(expected)),
        ))
    }
}

fn window(data: &[u8], offset: usize, limit: usize) -> Result<&[u8]> {
    data.get(offset..limit.min(data.len()))
        .ok_or_else(|| Error::corrupt(offset, "chunk starts past the end of its parent"))
}

fn prefix(window: &[u8], offset: usize) -> Result<([u8; 4], u32, u32)> {
    let fields = (tag_at(window, 0), u32_at(window, 4), u32_at(window, 8));
    let (Some(tag), Some(header_len), Some(third)) = fields else {
        return Err(Error::corrupt(offset, "truncated chunk header"));
    };
    Ok((tag, header_len, third))
}

pub(super) fn tag_name(tag: [u8; 4]) -> String {
    String::from_utf8_lossy(&tag).into_owned()
}
