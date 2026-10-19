//! mhlp/mhyp/mhip: playlists and their members. Offsets follow libgpod's `get_playlist` and
//! `get_mhip` (itdb_itunesdb.c).

use super::chunks::{Chunk, ListHead, MHIP, MHLP, MHYP, tag_name};
use super::mhod::{self, Mhod};
use super::raw::{RawMhod, RawPlaylist};
use crate::database::IpodPlaylist;
use crate::error::{Error, Result};

mod at {
    pub(super) const MHOD_COUNT: usize = 12;
    pub(super) const MHIP_COUNT: usize = 16;

    pub(super) const ENTRY_GROUP_FLAG: usize = 16;
    pub(super) const ENTRY_TRACK_ID: usize = 24;
}

pub(super) fn parse_list(data: &[u8], section: &Chunk) -> Result<Vec<IpodPlaylist>> {
    parse_raw_list(data, section)?
        .into_iter()
        .map(|playlist| {
            Ok(IpodPlaylist {
                id: playlist.id(),
                name: playlist.name()?,
                is_master: playlist.is_master(),
                is_podcasts: playlist.is_podcasts(),
                is_smart: playlist.is_smart(),
                track_ids: playlist.members,
            })
        })
        .collect()
}

pub(super) fn parse_raw_list(data: &[u8], section: &Chunk) -> Result<Vec<RawPlaylist>> {
    let list = ListHead::find(data, section, MHLP)?;
    let mut playlists = Vec::new();
    let mut offset = list.items_offset;
    while playlists.len() < list.count as usize && offset < section.end {
        let chunk = Chunk::read(data, offset, section.end)?;
        offset = chunk.end;
        if chunk.tag == MHYP {
            playlists.push(parse_playlist(data, &chunk)?);
        }
    }
    Ok(playlists)
}

fn parse_playlist(data: &[u8], chunk: &Chunk) -> Result<RawPlaylist> {
    let mut mhods = Vec::new();
    let mut offset = chunk.header_end();
    for _ in 0..chunk.u32(at::MHOD_COUNT).unwrap_or(0) {
        if offset >= chunk.end {
            break;
        }
        let mhod = Mhod::read(data, offset, chunk.end)?;
        // The sort indexes are rebuilt from the tracks on every write, so they are not kept.
        if !matches!(mhod.kind, mhod::INDEX | mhod::JUMP_TABLE) {
            let bytes = data.get(offset..mhod.end()).unwrap_or_default().to_vec();
            mhods.push(RawMhod {
                kind: mhod.kind,
                bytes,
            });
        }
        offset = mhod.end();
    }

    let mut entries = Vec::new();
    for index in 0..chunk.u32(at::MHIP_COUNT).unwrap_or(0) {
        if offset >= chunk.end {
            break;
        }
        let (entry, next) = parse_entry(data, offset, chunk.end, index)?;
        offset = next;
        entries.extend(entry);
    }
    entries.sort_by_key(|&(position, _)| position);
    Ok(RawPlaylist {
        header: chunk.header().to_vec(),
        mhods,
        members: entries.into_iter().map(|(_, track_id)| track_id).collect(),
    })
}

/// Returns the member as `(position, track id)` and where the next mhip starts. A member without a
/// position mhod keeps its file order (`index`). Podcast group headers yield no member.
fn parse_entry(
    data: &[u8],
    offset: usize,
    limit: usize,
    index: u32,
) -> Result<(Option<(u32, u32)>, usize)> {
    let chunk = Chunk::read(data, offset, limit)?;
    if chunk.tag != MHIP {
        return Err(Error::corrupt(
            offset,
            format!("expected mhip, found {}", tag_name(chunk.tag)),
        ));
    }
    let Some(track_id) = chunk.u32(at::ENTRY_TRACK_ID) else {
        return Err(Error::corrupt(offset, "mhip header has no track id"));
    };

    // iTunes up to 4.7 wrote total_len == header_len, leaving the mhods outside the chunk.
    let (mut mhod_offset, mhod_limit) = if chunk.body.is_empty() {
        (chunk.end, limit)
    } else {
        (chunk.header_end(), chunk.end)
    };
    let mut position = None;
    for _ in 0..chunk.u32(at::MHOD_COUNT).unwrap_or(0) {
        if mhod_offset >= mhod_limit {
            break;
        }
        let mhod = Mhod::read(data, mhod_offset, mhod_limit)?;
        mhod_offset = mhod.end();
        if mhod.kind == mhod::POSITION && position.is_none() {
            position = mhod.position();
        }
    }

    let is_group_header = chunk
        .u32(at::ENTRY_GROUP_FLAG)
        .is_some_and(|flag| flag != 0);
    let entry = (!is_group_header).then(|| (position.unwrap_or(index), track_id));
    Ok((entry, chunk.end.max(mhod_offset)))
}
