use crate::device::Ipod;
use crate::itunesdb::artwork::Artwork;

/// Whether `apply_sync` will write album art for this iPod. Covers are ignored on iPods without
/// album art (mini, shuffle, the original models) and on any whose thumbnail formats this crate
/// cannot produce.
pub fn artwork_supported(ipod: &Ipod) -> bool {
    Artwork::for_device(&ipod.mount_path, ipod.model.as_ref()).is_some()
}
