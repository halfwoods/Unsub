//! Add already-compatible local videos without claiming music-sync ownership of them.
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use unsub_ipod::video::{self, Protection, VideoKind};
use unsub_ipod::{Ipod, IpodFamily, IpodMediaKind, SyncTrack, VideoSyncTrack};
use unsub_playback::video::{VideoProfile, inspect};

use crate::devices::{self, DevicesState};
use crate::error::{CommandError, CommandResult};
use crate::jobs::{self, Kind};
use crate::sync::SYNC_EVENT;

const MAX_FILES: usize = 500;

/// A new selection stops older hashing work instead of leaving hundreds of stale file reads
/// running behind the sheet. The UI separately discards responses from superseded requests.
#[derive(Default)]
pub(crate) struct PreviewState(Mutex<Option<(String, Weak<AtomicBool>)>>);

impl PreviewState {
    fn begin(&self, request_id: &str) -> Arc<AtomicBool> {
        let mut previous = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cancel) = previous.take().and_then(|(_, weak)| weak.upgrade()) {
            cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *previous = Some((request_id.into(), Arc::downgrade(&cancel)));
        cancel
    }

    fn cancel(&self, request_id: &str) {
        let mut previous = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if previous.as_ref().is_some_and(|(id, _)| id == request_id)
            && let Some(cancel) = previous.take().and_then(|(_, weak)| weak.upgrade())
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn error(e: impl std::fmt::Display) -> CommandError {
    CommandError::from(e.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    path: String,
    title: String,
    bytes: Option<u64>,
    duration_ms: Option<u64>,
    reason: Option<String>,
    already_present: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    items: Vec<Item>,
    bytes: u64,
    free_bytes: u64,
    fits: bool,
    supported: bool,
    warning: Option<String>,
}

fn validate_request(paths: &[String], kind: VideoKind) -> CommandResult<()> {
    if paths.len() > MAX_FILES {
        return Err(error("Choose up to 500 videos at a time."));
    }
    if !matches!(kind, VideoKind::Movie | VideoKind::MusicVideo) {
        return Err(error("Choose Movies or Music Videos."));
    }
    Ok(())
}

fn fresh_ipod(app: &AppHandle, id: &str) -> CommandResult<Ipod> {
    let known = app.state::<DevicesState>().ipod(id)?;
    let current = unsub_ipod::open(&known.mount_path).map_err(error)?;
    if current.id != known.id {
        return Err(error("The iPod at this location has changed. Select it again before adding videos."));
    }
    Ok(current)
}

fn profile(ipod: &Ipod) -> VideoProfile {
    if ipod.model.as_ref().is_some_and(|model| model.family == IpodFamily::Video) {
        VideoProfile::LegacyVideo
    } else {
        VideoProfile::Classic
    }
}

fn local_source(path: &Path, mount: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("Choose a local file using Choose Videos….".into());
    }
    let path = fs::canonicalize(path).map_err(|e| format!("Cannot open this file: {e}"))?;
    let mount = fs::canonicalize(mount).map_err(|e| format!("The iPod is unavailable: {e}"))?;
    if path.starts_with(mount) {
        return Err("This file is already on the destination iPod. Choose an original file on your computer.".into());
    }
    let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err("Choose a nonempty, regular video file.".into());
    }
    if metadata.len() > u64::from(u32::MAX) {
        return Err("This file exceeds the iPod's 4 GB file limit. Convert it to a smaller file first.".into());
    }
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default();
    if !["mp4", "m4v", "mov"].iter().any(|ext| extension.eq_ignore_ascii_case(ext)) {
        return Err("Choose an iPod-compatible MP4, M4V or MOV file. Convert other formats first.".into());
    }
    Ok(path)
}

fn fingerprint(path: &Path, cancel: &AtomicBool) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Video checking stopped.".into());
        }
        let len = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if len == 0 {
            return Ok(format!("{:x}", hasher.finalize()));
        }
        hasher.update(&buffer[..len]);
    }
}

fn inspect_source(ipod: &Ipod, path: &str, kind: VideoKind, id: i64, cancel: &AtomicBool) -> Result<VideoSyncTrack, String> {
    let path = local_source(Path::new(path), &ipod.mount_path)?;
    let before = fs::metadata(&path).map_err(|e| e.to_string())?;
    let (protection, has_video) = video::sample_protection(&path).map_err(|e| format!("Cannot read this video: {e}"))?;
    if protection == Protection::Protected {
        return Err("This video is copy-protected. Unsub cannot add protected videos.".into());
    }
    if protection != Protection::Unprotected || !has_video {
        return Err("Cannot verify an unprotected video track in this file.".into());
    }
    let info = inspect(&path)?;
    if let Some(reason) = info.incompatibility(profile(ipod)) {
        return Err(reason);
    }
    let sha256 = fingerprint(&path, cancel)?;
    let after = fs::metadata(&path).map_err(|e| e.to_string())?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("This file changed while it was checked. Choose it again after editing finishes.".into());
    }
    let title = if info.title.trim().is_empty() { path.file_stem().unwrap_or_default().to_string_lossy().into_owned() } else { info.title };
    Ok(VideoSyncTrack {
        track: SyncTrack {
            source_path: path,
            library_id: id,
            title,
            artist: info.artist,
            album: info.album,
            album_artist: String::new(),
            genre: String::new(),
            composer: String::new(),
            year: None,
            track_number: None,
            track_count: None,
            disc_number: None,
            disc_count: None,
            duration_ms: info.duration_ms,
            bitrate_kbps: Some(info.bitrate_kbps),
            sample_rate: info.sample_rate,
            rating: 0,
            play_count: 0,
            last_played: None,
            date_added: crate::playback::now_unix(),
            media_kind: IpodMediaKind::Video,
            compilation: false,
            sort_title: None,
            sort_artist: None,
            sort_album: None,
            sort_album_artist: None,
            artwork_key: None,
            sound_check: None,
        },
        kind,
        sha256,
    })
}

fn preview_with_cancel(ipod: &Ipod, paths: &[String], kind: VideoKind, cancel: &AtomicBool) -> CommandResult<Preview> {
    validate_request(paths, kind)?;
    let mut preview = Preview {
        items: Vec::new(),
        bytes: 0,
        free_bytes: unsub_ipod::free_space(&ipod.mount_path).map_err(error)?,
        fits: false,
        supported: false,
        warning: None,
    };
    if let Err(e) = unsub_ipod::video_add_supported(ipod) {
        preview.warning = Some(format!("Videos cannot be added to this iPod: {e}"));
        return Ok(preview);
    }
    preview.supported = true;
    let mut tracks = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(error("Video checks were replaced by a new selection."));
        }
        let mut item = Item {
            path: path.clone(),
            title: Path::new(path).file_name().unwrap_or_default().to_string_lossy().into_owned(),
            bytes: None,
            duration_ms: None,
            reason: None,
            already_present: false,
        };
        match inspect_source(ipod, path, kind, index as i64, cancel) {
            Ok(track) => {
                item.title = track.track.title.clone();
                item.duration_ms = Some(track.track.duration_ms);
                item.bytes = fs::metadata(&track.track.source_path).ok().map(|meta| meta.len());
                tracks.push(track);
            }
            Err(reason) => item.reason = Some(reason),
        }
        preview.items.push(item);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(error("Video checks were replaced by a new selection."));
    }
    let plan = unsub_ipod::plan_video_additions(ipod, &tracks).map_err(error)?;
    for id in &plan.duplicate {
        if let Some(item) = preview.items.get_mut(*id as usize) {
            item.already_present = true;
        }
    }
    preview.bytes = plan.bytes_needed;
    preview.fits = plan.fits;
    Ok(preview)
}

#[tauri::command]
pub async fn device_video_add_preview(
    app: AppHandle,
    device_id: String,
    paths: Vec<String>,
    kind: VideoKind,
    request_id: String,
) -> CommandResult<Preview> {
    validate_request(&paths, kind)?;
    let cancel = app.state::<PreviewState>().begin(&request_id);
    tauri::async_runtime::spawn_blocking(move || preview_with_cancel(&fresh_ipod(&app, &device_id)?, &paths, kind, &cancel))
        .await
        .map_err(error)?
}

#[tauri::command]
pub fn device_video_add_cancel_preview(app: AppHandle, request_id: String) {
    app.state::<PreviewState>().cancel(&request_id);
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    device_id: String,
    stage: &'static str,
    done: u64,
    total: u64,
    current: String,
}

#[tauri::command]
pub fn device_video_add(app: AppHandle, device_id: String, paths: Vec<String>, kind: VideoKind) -> CommandResult<()> {
    validate_request(&paths, kind)?;
    if paths.is_empty() {
        return Err(error("Choose at least one compatible video to add."));
    }
    let status = |current: &str| Status {
        device_id: device_id.clone(),
        stage: "addingVideos",
        done: 0,
        total: paths.len() as u64,
        current: current.into(),
    };
    let job =
        jobs::Job::new(Kind::Sync, SYNC_EVENT).resource(&device_id).starting(status("Checking videos…")).changed(devices::CHANGED_EVENT);
    jobs::start(&app, job, move |app, ctx| {
        let run = || -> CommandResult<String> {
            let ipod = fresh_ipod(app, &device_id)?;
            unsub_ipod::video_add_supported(&ipod).map_err(error)?;
            let mut tracks = Vec::with_capacity(paths.len());
            for (index, path) in paths.iter().enumerate() {
                if ctx.cancelled() {
                    return Ok("Adding videos stopped before any files were copied.".into());
                }
                ctx.progress(
                    index as u64,
                    Some(paths.len() as u64),
                    ("checking", index),
                    Status {
                        device_id: device_id.clone(),
                        stage: "addingVideos",
                        done: index as u64,
                        total: paths.len() as u64,
                        current: format!("Checking {}…", Path::new(path).file_name().unwrap_or_default().to_string_lossy()),
                    },
                );
                tracks.push(inspect_source(&ipod, path, kind, index as i64, ctx.cancel_flag()).map_err(error)?);
            }
            let result = unsub_ipod::apply_video_additions(&ipod, &tracks, ctx.cancel_flag(), &mut |progress| {
                ctx.progress(
                    progress.done,
                    Some(progress.total),
                    format!("{:?}", progress.stage),
                    Status {
                        device_id: device_id.clone(),
                        stage: if progress.stage == unsub_ipod::SyncStage::WritingDatabase { "writingDatabase" } else { "addingVideos" },
                        done: progress.done,
                        total: progress.total,
                        current: progress.current,
                    },
                );
            });
            // Even a partial result may have committed a complete batch; always refresh it.
            let refreshed = app.state::<DevicesState>().reconnect(&ipod.mount_path);
            let report = result.map_err(error)?;
            refreshed.map_err(error)?;
            Ok(format!(
                "{}{} video(s) added; {} already present.",
                if report.cancelled { "Adding videos stopped. " } else { "" },
                report.added,
                report.duplicates
            ))
        };
        ctx.notice(match run() {
            Ok(message) => message,
            Err(e) => format!("Adding videos did not finish: {e}"),
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(ipod: &Ipod, paths: &[String], kind: VideoKind) -> CommandResult<Preview> {
        preview_with_cancel(ipod, paths, kind, &AtomicBool::new(false))
    }

    #[test]
    fn newer_previews_cancel_stale_hashing() {
        let state = PreviewState::default();
        let first = state.begin("first");
        let second = state.begin("second");
        assert!(first.load(Ordering::Relaxed));
        assert!(!second.load(Ordering::Relaxed));
        state.cancel("first");
        assert!(!second.load(Ordering::Relaxed), "stale disposal must not cancel the new request");
        state.cancel("second");
        assert!(second.load(Ordering::Relaxed));
        drop(second);
        assert!(!state.begin("third").load(Ordering::Relaxed));
    }

    fn test_ipod(root: &Path) -> Ipod {
        let mount = root.join("Test iPod");
        fs::create_dir_all(mount.join("iPod_Control/Device")).unwrap();
        fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        fs::write(mount.join("iPod_Control/Device/SysInfo"), "ModelNumStr: xC293\nFirewireGuid: 0x000A27001A2B3C4D\n").unwrap();
        unsub_ipod::open(&mount).unwrap()
    }

    fn fixture(name: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/playback/tests/fixtures/video")
            .join(name)
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn preview_to_add_to_repeat_preview_uses_real_streams_and_preserves_source() {
        let dir = tempfile::tempdir().unwrap();
        let ipod = test_ipod(dir.path());
        let valid = fixture("baseline.mp4");
        let invalid = fixture("high-profile.mp4");
        let cancel = AtomicBool::new(false);
        let before = fingerprint(Path::new(&valid), &cancel).unwrap();
        let result = preview(&ipod, &[valid.clone(), invalid], VideoKind::MusicVideo).unwrap();
        assert!(result.supported && result.fits);
        assert!(result.items[0].reason.is_none(), "{result:?}");
        assert!(result.items[1].reason.is_some());
        assert!(!ipod.mount_path.join("iPod_Control/iTunes/iTunesDB").exists(), "preview must not write");
        let track = inspect_source(&ipod, &valid, VideoKind::MusicVideo, 0, &cancel).unwrap();
        let added = unsub_ipod::apply_video_additions(&ipod, &[track], &cancel, &mut |_| {}).unwrap();
        assert_eq!(added.added, 1);
        assert_eq!(added.database.tracks[0].video_kind, Some(VideoKind::MusicVideo));
        assert_eq!(fingerprint(Path::new(&valid), &cancel).unwrap(), before);
        let repeat = preview(&ipod, &[valid], VideoKind::Movie).unwrap();
        assert!(repeat.items[0].already_present);
        assert_eq!(repeat.bytes, 0);
    }

    #[test]
    fn unsupported_devices_explain_capability_without_inspecting_sources() {
        let dir = tempfile::tempdir().unwrap();
        let mut ipod = test_ipod(dir.path());
        ipod.model = None;
        let result = preview(&ipod, &["/not/a/real/file.mp4".into()], VideoKind::Movie).unwrap();
        assert!(!result.supported);
        assert!(result.warning.is_some());
        assert!(result.items.is_empty());
    }

    #[test]
    fn malformed_and_protected_inputs_are_rejected_before_video_copy() {
        let dir = tempfile::tempdir().unwrap();
        let ipod = test_ipod(dir.path());
        let path = dir.path().join("bad.mp4");
        fs::write(&path, b"not a video").unwrap();
        let result = preview(&ipod, &[path.to_string_lossy().into_owned()], VideoKind::Movie).unwrap();
        assert!(result.items[0].reason.is_some());
        assert_eq!(result.bytes, 0);
        // Change the actual MP4 sample entry; container extension alone must not authorize it.
        let mut bytes = fs::read(fixture("baseline.mp4")).unwrap();
        let at = bytes.windows(4).position(|tag| tag == b"avc1").unwrap();
        bytes[at..at + 4].copy_from_slice(b"encv");
        // The first occurrence can be an ftyp brand; replace the sample-entry occurrence too.
        for at in (0..bytes.len().saturating_sub(4)).collect::<Vec<_>>() {
            if &bytes[at..at + 4] == b"avc1" {
                bytes[at..at + 4].copy_from_slice(b"encv");
            }
        }
        fs::write(&path, bytes).unwrap();
        let reason = inspect_source(&ipod, path.to_str().unwrap(), VideoKind::Movie, 0, &AtomicBool::new(false)).unwrap_err();
        assert!(reason.contains("copy-protected"), "{reason}");
    }

    #[test]
    fn requests_are_bounded_and_only_supported_categories_are_accepted() {
        assert!(validate_request(&[], VideoKind::Movie).is_ok());
        assert!(validate_request(&[], VideoKind::MusicVideo).is_ok());
        assert!(validate_request(&[], VideoKind::Tv).is_err());
        assert!(validate_request(&vec![String::new(); 501], VideoKind::Movie).is_err());
    }

    #[test]
    fn local_source_excludes_device_files_empty_files_and_other_formats() {
        let dir = tempfile::tempdir().unwrap();
        let mount = dir.path().join("ipod");
        fs::create_dir(&mount).unwrap();
        let inside = mount.join("movie.mp4");
        fs::write(&inside, b"video").unwrap();
        assert!(local_source(&inside, &mount).unwrap_err().contains("destination iPod"));
        assert!(local_source(Path::new("https://example.org/movie.mp4"), &mount).is_err());
        let file = dir.path().join("movie.mp4");
        fs::write(&file, []).unwrap();
        assert!(local_source(&file, &mount).is_err());
        fs::write(&file, b"video").unwrap();
        assert!(local_source(&file, &mount).is_ok());
        let other = dir.path().join("movie.mkv");
        fs::write(&other, b"video").unwrap();
        assert!(local_source(&other, &mount).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_hide_a_source_on_the_destination_device() {
        let dir = tempfile::tempdir().unwrap();
        let mount = dir.path().join("ipod");
        fs::create_dir(&mount).unwrap();
        let inside = mount.join("movie.mp4");
        fs::write(&inside, b"video").unwrap();
        let link = dir.path().join("movie.mp4");
        std::os::unix::fs::symlink(&inside, &link).unwrap();
        assert!(local_source(&link, &mount).is_err());
    }

    #[test]
    fn fingerprint_is_content_based_and_cancellable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("movie.mp4");
        fs::write(&file, b"abc").unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(fingerprint(&file, &cancel).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        cancel.store(true, Ordering::Relaxed);
        assert!(fingerprint(&file, &cancel).is_err());
    }
}
