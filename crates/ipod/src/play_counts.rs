//! `iPod_Control/iTunes/Play Counts`: what the iPod recorded since the last sync, one entry per
//! track in iTunesDB order. Layout follows libgpod's `playcounts_read` (itdb_itunesdb.c).

use std::fs;
use std::path::Path;

use crate::bytes::{tag_at, u32_at};
use crate::database::IpodTrack;
use crate::device::itunes_dir;
use crate::error::{Error, Result};
use crate::time::mac_to_unix;

const MIN_HEADER_LEN: usize = 0x60;
/// Firmware 1.3 wrote 12-byte entries; later firmware appended rating, an unknown field, then skips.
const MIN_ENTRY_LEN: usize = 0x0c;
const RATING_ENTRY_LEN: usize = 0x10;
const SKIP_ENTRY_LEN: usize = 0x1c;

struct Entry {
    play_count: u32,
    last_played: Option<i64>,
    rating: Option<u32>,
    skip_count: u32,
}

/// Best effort: a missing, empty or damaged file leaves the tracks as the database has them.
pub(crate) fn merge(mount: &Path, tracks: &mut [IpodTrack]) {
    let Ok(bytes) = fs::read(itunes_dir(mount).join("Play Counts")) else {
        return;
    };
    let Ok(entries) = parse(&bytes) else { return };
    for (track, entry) in tracks.iter_mut().zip(entries) {
        track.play_count = track.play_count.saturating_add(entry.play_count);
        track.skip_count = track.skip_count.saturating_add(entry.skip_count);
        if entry.last_played.is_some() {
            track.last_played = entry.last_played;
        }
        // Newer firmware repeats the database rating here, so only a nonzero value is a change
        // that can be told apart from "unchanged"; clearing a rating on the iPod is not detected.
        if let Some(rating) = entry.rating.filter(|&rating| rating != 0) {
            track.rating = u8::try_from(rating.min(100)).unwrap_or(100);
        }
    }
}

fn parse(data: &[u8]) -> Result<Vec<Entry>> {
    // Left empty by an interrupted sync or fsck; libgpod treats that as "no counts".
    if data.len() < MIN_HEADER_LEN {
        return Ok(Vec::new());
    }
    if tag_at(data, 0) != Some(*b"mhdp") {
        return Err(Error::corrupt(
            0,
            "not a Play Counts file: missing mhdp header",
        ));
    }
    let (Some(header_len), Some(entry_len), Some(count)) =
        (u32_at(data, 4), u32_at(data, 8), u32_at(data, 12))
    else {
        return Err(Error::corrupt(0, "truncated Play Counts header"));
    };
    let (header_len, entry_len) = (header_len as usize, entry_len as usize);
    if header_len < MIN_HEADER_LEN || entry_len < MIN_ENTRY_LEN {
        return Err(Error::corrupt(
            4,
            "Play Counts header or entry length is too small",
        ));
    }

    (0..count as usize)
        .map(|index| {
            let start = index
                .checked_mul(entry_len)
                .and_then(|offset| offset.checked_add(header_len));
            let entry = start
                .and_then(|start| data.get(start..start.checked_add(entry_len)?))
                .ok_or_else(|| {
                    Error::corrupt(
                        header_len,
                        format!("Play Counts entry {index} is truncated"),
                    )
                })?;
            Ok(Entry {
                play_count: u32_at(entry, 0).unwrap_or(0),
                last_played: u32_at(entry, 4).and_then(mac_to_unix),
                rating: if entry_len >= RATING_ENTRY_LEN {
                    u32_at(entry, 12)
                } else {
                    None
                },
                skip_count: if entry_len >= SKIP_ENTRY_LEN {
                    u32_at(entry, 20).unwrap_or(0)
                } else {
                    0
                },
            })
        })
        .collect()
}
