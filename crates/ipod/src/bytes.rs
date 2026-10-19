//! Little-endian field readers that return `None` instead of indexing past the end.

pub(crate) fn u8_at(bytes: &[u8], at: usize) -> Option<u8> {
    bytes.get(at).copied()
}

pub(crate) fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    let field = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(field.try_into().ok()?))
}

pub(crate) fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let field = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(field.try_into().ok()?))
}

pub(crate) fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    let field = bytes.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(field.try_into().ok()?))
}

pub(crate) fn tag_at(data: &[u8], at: usize) -> Option<[u8; 4]> {
    data.get(at..)?.get(..4)?.try_into().ok()
}

/// Overwrites `value.len()` bytes at `at`. A field that lies beyond a short header was never part
/// of that iTunes version's layout, so the write is skipped rather than extending the header.
pub(crate) fn put(buf: &mut [u8], at: usize, value: &[u8]) {
    if let Some(field) = at
        .checked_add(value.len())
        .and_then(|end| buf.get_mut(at..end))
    {
        field.copy_from_slice(value);
    }
}
