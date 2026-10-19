//! mhlp/mhyp/mhip: playlists and their members. Offsets follow libgpod's `get_playlist` and
//! `get_mhip` (itdb_itunesdb.c).

use super::chunks::{Chunk, ListHead, MHIP, MHLP, MHYP, tag_name};
use super::mhod::{self, Mhod};
use crate::database::IpodPlaylist;
use crate::error::{Error, Result};

mod at {
    pub(super) const MHOD_COUNT: usize = 12;
    pub(super) const MHIP_COUNT: usize = 16;
    pub(super) const TYPE: usize = 20;
    pub(super) const ID: usize = 28;
    pub(super) const PODCAST_FLAG: usize = 42;

    pub(super) const ENTRY_GROUP_FLAG: usize = 16;
    pub(super) const ENTRY_TRACK_ID: usize = 24;
}

/// `type` 1 in an mhyp marks the master playlist (libgpod's `ITDB_PL_TYPE_MPL`).
const MASTER_TYPE: u8 = 1;
/// `podcastflag` 1 marks the Podcasts playlist (libgpod's `ITDB_PL_FLAG_PODCASTS`).
const PODCASTS_FLAG: u16 = 1;

pub(super) fn parse_list(data: &[u8], section: &Chunk) -> Result<Vec<IpodPlaylist>> {
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

fn parse_playlist(data: &[u8], chunk: &Chunk) -> Result<IpodPlaylist> {
    let mut playlist = IpodPlaylist {
        id: chunk.u64(at::ID).unwrap_or(0),
        is_master: chunk.u8(at::TYPE) == Some(MASTER_TYPE),
        is_podcasts: chunk.u16(at::PODCAST_FLAG) == Some(PODCASTS_FLAG),
        ..IpodPlaylist::default()
    };

    let mut offset = chunk.header_end();
    for _ in 0..chunk.u32(at::MHOD_COUNT).unwrap_or(0) {
        if offset >= chunk.end {
            break;
        }
        let mhod = Mhod::read(data, offset, chunk.end)?;
        offset = mhod.end();
        match mhod.kind {
            // iTunes sometimes writes two titles; the last one wins, as in libgpod.
            mhod::TITLE => playlist.name = mhod.text()?,
            mhod::SMART_PREFS | mhod::SMART_RULES => playlist.is_smart = true,
            _ => {}
        }
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
    playlist.track_ids = entries.into_iter().map(|(_, track_id)| track_id).collect();
    Ok(playlist)
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
