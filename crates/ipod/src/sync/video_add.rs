//! Add-only video copies, deliberately outside normal music synchronization ownership.
use std::collections::HashSet;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use super::mapping::Mapping;
use super::plan::reserve_bytes;
use super::{
    SyncPlan, SyncProgress, SyncTrack, checksum_key, require_blank_or_database,
    require_guid_matches,
};
use crate::device::{database_exists, database_path};
use crate::video::{Protection, VideoKind, sample_protection};
use crate::{Database, Error, Ipod, IpodFamily, IpodMediaKind, Result, read_database};

/// A local video already inspected for the destination's codec and playback limits by the caller.
/// `sha256` identifies those inspected bytes; copying verifies it before database registration.
/// The wrapped track's `library_id` is unique only within this operation, never music ownership.
#[derive(Debug, Clone)]
pub struct VideoSyncTrack {
    pub track: SyncTrack,
    pub kind: VideoKind,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoAddPlan {
    pub add: Vec<i64>,
    pub duplicate: Vec<i64>,
    pub bytes_needed: u64,
    pub fits: bool,
}

#[derive(Debug)]
pub struct VideoAddResult {
    pub database: Database,
    pub added: usize,
    pub duplicates: usize,
    /// Completed copies are committed; an in-flight partial file is removed.
    pub cancelled: bool,
}

/// Check both video hardware and the existing safe database-writing gates. This performs no
/// writes, and never relaxes checksum/GUID requirements for adding videos.
pub fn video_add_supported(ipod: &Ipod) -> Result<()> {
    let capable = ipod.model.as_ref().is_some_and(|model| {
        matches!(model.family, IpodFamily::Video | IpodFamily::Classic)
            || (model.family == IpodFamily::Nano && (3..=5).contains(&model.generation))
    });
    if !capable {
        return Err(Error::Unsupported(
            "this iPod does not support video playback",
        ));
    }
    let key = checksum_key(ipod)?;
    require_blank_or_database(ipod)?;
    if ipod.guid_from_usb && database_exists(ipod) {
        let path = database_path(&ipod.mount_path, ipod.database);
        let bytes = fs::read(&path).map_err(|source| Error::io(path, source))?;
        require_guid_matches(ipod, key, &bytes)?;
    }
    Ok(())
}

/// Inspect an additive batch and available space. Nothing is removed to make room. Duplicate
/// content copied by this feature is skipped only while its database entry and file remain.
pub fn plan_video_additions(ipod: &Ipod, videos: &[VideoSyncTrack]) -> Result<VideoAddPlan> {
    video_add_supported(ipod)?;
    if videos.len() > 500 {
        return Err(Error::SyncRefused(
            "select at most 500 videos at a time".into(),
        ));
    }
    let current = read_database(ipod)?;
    let mapping = Mapping::load(&ipod.mount_path)?;
    let live: HashSet<u64> = current
        .tracks
        .iter()
        .filter(|track| {
            track.video_kind.is_some()
                && fs::metadata(&track.path)
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() == track.file_size)
        })
        .map(|track| track.dbid)
        .collect();
    let mut hashes: HashSet<String> = mapping
        .video_copies
        .iter()
        .filter(|copy| live.contains(&copy.dbid))
        .map(|copy| copy.sha256.clone())
        .collect();
    let mount =
        fs::canonicalize(&ipod.mount_path).map_err(|source| Error::io(&ipod.mount_path, source))?;
    let mut ids = HashSet::new();
    let (mut add, mut duplicate, mut bytes_needed) = (Vec::new(), Vec::new(), 0u64);
    for video in videos {
        let source = &video.track.source_path;
        if !ids.insert(video.track.library_id) {
            return Err(Error::SyncRefused(
                "video identifiers must be unique within a batch".into(),
            ));
        }
        if !matches!(video.kind, VideoKind::Movie | VideoKind::MusicVideo)
            || video.track.media_kind != IpodMediaKind::Video
        {
            return Err(Error::SyncRefused(
                "choose Movie or Music Video for new videos".into(),
            ));
        }
        if video.sha256.len() != 64
            || !video
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::SyncRefused(
                "the video has no valid inspected SHA-256 fingerprint".into(),
            ));
        }
        let canonical = fs::canonicalize(source).map_err(|error| Error::io(source, error))?;
        if canonical.starts_with(&mount) {
            return Err(Error::SyncRefused(
                "choose a video on your computer, outside the destination iPod".into(),
            ));
        }
        let metadata = fs::metadata(&canonical).map_err(|error| Error::io(source, error))?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > u64::from(u32::MAX) {
            return Err(Error::SyncRefused(
                "videos must be nonempty regular files smaller than 4 GiB".into(),
            ));
        }
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "mp4" | "m4v" | "mov") {
            return Err(Error::SyncRefused(
                "use an iPod-compatible MP4, M4V or MOV video".into(),
            ));
        }
        let (protection, has_video) =
            sample_protection(&canonical).map_err(|error| Error::io(source, error))?;
        if protection != Protection::Unprotected || !has_video {
            return Err(Error::SyncRefused(
                "the file must contain an unprotected video stream".into(),
            ));
        }
        if hashes.insert(video.sha256.clone()) {
            add.push(video.track.library_id);
            bytes_needed = bytes_needed.saturating_add(metadata.len());
        } else {
            duplicate.push(video.track.library_id);
        }
    }
    let available = fs4::available_space(&ipod.mount_path)
        .map_err(|source| Error::io(&ipod.mount_path, source))?;
    let fits = add.is_empty()
        || bytes_needed.saturating_add(reserve_bytes(&ipod.mount_path, ipod)) <= available;
    Ok(VideoAddPlan {
        add,
        duplicate,
        bytes_needed,
        fits,
    })
}

/// Replan and copy an additive batch with the sync writer's backup, rollback, snapshot and
/// checkpoint guarantees. Cancellation commits completed files and reports `cancelled`; errors
/// remove the current uncommitted batch, with `Error::Partial` identifying earlier checkpoints.
/// Codec/profile validation belongs to the caller; this layer additionally checks video atoms,
/// source identity and SHA-256 while copying. It does not transcode or modify source files.
pub fn apply_video_additions(
    ipod: &Ipod,
    videos: &[VideoSyncTrack],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(SyncProgress),
) -> Result<VideoAddResult> {
    let planned = plan_video_additions(ipod, videos)?;
    if planned.add.is_empty() || cancel.load(Ordering::Relaxed) {
        return Ok(VideoAddResult {
            database: read_database(ipod)?,
            added: 0,
            duplicates: planned.duplicate.len(),
            cancelled: cancel.load(Ordering::Relaxed),
        });
    }
    let plan = SyncPlan {
        add: planned.add,
        update: Vec::new(),
        remove: Vec::new(),
        bytes_needed: planned.bytes_needed,
        fits: planned.fits,
        removals_first: false,
    };
    super::apply::apply_video_additions(
        ipod,
        &plan,
        videos,
        planned.duplicate.len(),
        cancel,
        progress,
    )
}
