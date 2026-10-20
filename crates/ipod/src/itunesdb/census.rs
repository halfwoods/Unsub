//! Counts every chunk in a database by kind, so a rewrite can be checked for chunks it dropped.

use std::collections::BTreeMap;

use super::chunks::{Chunk, ListHead, MHIP, MHLA, MHLI, MHLP, MHLT, tag_name};
use super::{MHSD_TYPE_AT, database_chunk, sections};
use crate::error::Result;

const MANAGED_SECTIONS: [u32; 5] = [1, 2, 3, 4, 8];
const MHOD_TYPE_AT: usize = 12;

pub(crate) type Census = BTreeMap<String, u64>;

pub(crate) fn census(data: &[u8]) -> Result<Census> {
    let database = database_chunk(data)?;
    let mut counts = Census::new();
    for section in sections(data, &database)? {
        let kind = section.u32(MHSD_TYPE_AT).unwrap_or(0);
        *counts.entry(format!("mhsd:{kind}")).or_default() += 1;
        // Sections the writer copies verbatim cannot lose anything, so their insides are not walked.
        if MANAGED_SECTIONS.contains(&kind) {
            walk_section(data, &section, &mut counts)?;
        }
    }
    Ok(counts)
}

/// A section holds one list header followed by that many sibling items; anything else is a chunk
/// the writer would not reproduce.
fn walk_section(data: &[u8], section: &Chunk, counts: &mut Census) -> Result<()> {
    let mut offset = section.header_end();
    let mut items_left = 0u32;
    while offset < section.end {
        let list_tag = [MHLT, MHLP, MHLA, MHLI]
            .into_iter()
            .find(|tag| crate::bytes::tag_at(data, offset) == Some(*tag));
        if let Some(tag) = list_tag {
            let list = ListHead::read(data, offset, section.end, tag)?;
            *counts.entry(tag_name(tag)).or_default() += 1;
            items_left = list.count;
            offset = list.items_offset;
            continue;
        }
        let chunk = Chunk::read(data, offset, section.end)?;
        offset = chunk.end;
        if items_left == 0 {
            *counts
                .entry(format!("unexpected:{}", tag_name(chunk.tag)))
                .or_default() += 1;
            continue;
        }
        items_left -= 1;
        walk_chunk(data, &chunk, counts)?;
    }
    Ok(())
}

fn walk_chunk(data: &[u8], chunk: &Chunk, counts: &mut Census) -> Result<()> {
    *counts.entry(label(chunk)).or_default() += 1;
    let mut offset = chunk.header_end();
    while offset < chunk.end {
        let child = Chunk::read(data, offset, chunk.end)?;
        offset = child.end;
        if child.tag == MHIP {
            walk_chunk(data, &child, counts)?;
        } else {
            *counts.entry(label(&child)).or_default() += 1;
        }
    }
    Ok(())
}

fn label(chunk: &Chunk) -> String {
    let name = tag_name(chunk.tag);
    match (name.as_str(), chunk.u32(MHOD_TYPE_AT)) {
        ("mhod", Some(kind)) => format!("mhod:{kind}"),
        _ => name,
    }
}
