use std::collections::HashMap;
use std::io;
use std::mem::discriminant;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use unsub_devicesync::{Device as Player, Profile, SyncItem};
use unsub_library::Track;
use unsub_playback::{Target, TranscodeOptions};

use super::{Chosen, FALLBACK_AAC_KBPS, SyncState, fresh_work_dir, is_lossless, select, to_error};
use crate::devices::{self, DevicesState};
use crate::error::CommandResult;
use crate::jobs::Ctx;
use crate::library::LibraryState;

/// What a player gets instead of a file it can't play: the profile's first format, made from
/// lossless sources losslessly where that format allows. `None` copies the file as it is.
pub(super) fn player_target(track: &Track, profile: &Profile) -> Option<(Target, &'static str)> {
    let extension = std::path::Path::new(&track.path).extension()?.to_str()?.to_ascii_lowercase();
    let within = profile.max_sample_rate.is_none_or(|max| track.sample_rate.is_none_or(|rate| rate <= max))
        && profile.max_bit_depth.is_none_or(|max| track.bit_depth.is_none_or(|bits| bits <= max));
    if profile.formats.is_empty() || (profile.formats.contains(&extension) && within) {
        return None;
    }
    let lossless = is_lossless(track);
    Some(match profile.formats.first().map(String::as_str) {
        Some("flac") if lossless => (Target::Flac, "flac"),
        Some("m4a") if lossless => (Target::Alac, "m4a"),
        Some("m4a" | "aac") => (Target::Aac { kbps: FALLBACK_AAC_KBPS }, "m4a"),
        Some("opus") => (Target::Opus { kbps: 160 }, "opus"),
        // MP3 plays on everything, so it's the answer for any format the transcoder can't write.
        _ => (Target::Mp3 { kbps: 320 }, "mp3"),
    })
}

fn sync_item(track: &Track) -> SyncItem {
    SyncItem {
        library_id: track.id,
        source: PathBuf::from(&track.path),
        album_artist: if track.album_artist.is_empty() { track.artist.clone() } else { track.album_artist.clone() },
        album: track.album.clone(),
        title: track.title.clone(),
        disc: track.disc_number,
        track: track.track_number,
        size: track.file_size,
        mtime: track.date_modified,
    }
}

pub(super) fn for_player(chosen: &Chosen) -> (Vec<SyncItem>, Vec<unsub_devicesync::SyncPlaylist>) {
    let items = chosen.tracks.iter().map(sync_item).collect();
    let playlists =
        chosen.playlists.iter().map(|(name, items)| unsub_devicesync::SyncPlaylist { name: name.clone(), items: items.clone() }).collect();
    (items, playlists)
}

/// Copies the chosen songs onto a non-Apple player as files and playlists. Its manifest records
/// each copy as it lands, so a stopped sync picks up where it left off next time.
pub(super) fn run_player_sync(app: &AppHandle, ctx: &Ctx, player: &Player) -> CommandResult<()> {
    let settings = app.state::<SyncState>().settings_for(&player.id);
    let chosen = select(&app.state::<LibraryState>().queries(), &settings, false)?;
    let work_dir = fresh_work_dir(app)?;
    let summary = sync_to_player(player, &chosen, &work_dir, ctx.cancel_flag(), &mut |progress| {
        let status = PlayerSyncStatus { device_id: player.id.clone(), progress };
        // Each song is its own stage: one report is made per song, before its slow conversion, and must show.
        let stage = (discriminant(&status.progress.stage), status.progress.done);
        ctx.progress(status.progress.done, Some(status.progress.total), stage, status);
    })?;
    app.state::<DevicesState>().rediscover_players();
    let _ = app.emit(devices::CHANGED_EVENT, ());
    // Songs the player couldn't take are the one thing worth reporting after a good sync.
    if !summary.skipped.is_empty() {
        let count = summary.skipped.len();
        let message = format!("Synced, but {count} {} couldn't be converted for the player", if count == 1 { "song" } else { "songs" });
        ctx.notice(message);
    }
    Ok(())
}

fn sync_to_player(
    player: &Player,
    chosen: &Chosen,
    work_dir: &std::path::Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(unsub_devicesync::SyncProgress),
) -> CommandResult<unsub_devicesync::SyncSummary> {
    let (items, playlists) = for_player(chosen);
    let plan = unsub_devicesync::plan(player, &items, &playlists).map_err(to_error)?;
    let targets: HashMap<i64, (Target, &'static str)> =
        chosen.tracks.iter().filter_map(|t| player_target(t, &player.profile).map(|target| (t.id, target))).collect();
    let mut previous: Option<PathBuf> = None;
    let mut prepare = |item: &SyncItem, profile: &Profile| -> io::Result<PathBuf> {
        if let Some(done) = previous.take() {
            let _ = std::fs::remove_file(done);
        }
        let Some(&(target, extension)) = targets.get(&item.library_id) else {
            return Ok(item.source.clone());
        };
        let output = work_dir.join(format!("{}.{extension}", item.library_id));
        let options =
            TranscodeOptions { target, max_sample_rate: profile.max_sample_rate, max_bit_depth: profile.max_bit_depth, keep_tags: true };
        unsub_playback::transcode_file(&item.source, &output, &options, &mut |_, _| {}, cancel)
            .map_err(|error| io::Error::other(error.to_string()))?;
        previous = Some(output.clone());
        Ok(output)
    };
    let result = unsub_devicesync::apply(player, &plan, &items, &playlists, &mut prepare, progress, cancel);
    if let Some(done) = previous {
        let _ = std::fs::remove_file(done);
    }
    result.map_err(to_error)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayerSyncStatus {
    device_id: String,
    #[serde(flatten)]
    progress: unsub_devicesync::SyncProgress,
}

#[cfg(test)]
mod tests {
    use unsub_library::MediaKind;

    use super::*;
    use crate::test_support::track;

    #[test]
    fn players_get_files_they_play_as_they_are_and_the_rest_in_their_first_format() {
        let mp3_only = Profile { formats: vec!["mp3".into()], ..Profile::mass_storage() };
        let flac = Track { path: "/music/a.flac".into(), ..track(1, "FLAC", MediaKind::Music) };
        let mp3 = Track { path: "/music/b.mp3".into(), ..track(2, "MP3", MediaKind::Music) };
        assert!(matches!(player_target(&flac, &mp3_only), Some((Target::Mp3 { kbps: 320 }, "mp3"))));
        assert!(player_target(&mp3, &mp3_only).is_none());

        let lossless_first =
            Profile { formats: vec!["flac".into(), "mp3".into()], max_sample_rate: Some(48_000), ..Profile::mass_storage() };
        let hires = Track { sample_rate: Some(96_000), ..flac.clone() };
        assert!(player_target(&flac, &lossless_first).is_none());
        assert!(matches!(player_target(&hires, &lossless_first), Some((Target::Flac, "flac"))));

        let anything = Profile { formats: Vec::new(), ..Profile::mass_storage() };
        assert!(player_target(&hires, &anything).is_none());
    }

    /// A real sync onto a folder posing as an MP3-only player: the FLAC is converted, the MP3
    /// copied as it is, the playlist written, and nothing converted is left behind.
    #[test]
    fn a_player_gets_its_songs_converted_copied_and_listed() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/playback/tests/fixtures");
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let profile = Profile { formats: vec!["mp3".into()], ..Profile::mass_storage() };
        let player = Player {
            id: "test".into(),
            name: "Test Player".into(),
            kind: unsub_devicesync::DeviceKind::MassStorage,
            root: unsub_devicesync::DeviceRoot::Path(root.path().to_path_buf()),
            capacity_bytes: 1 << 40,
            free_bytes: 1 << 40,
            profile,
        };
        let file = |name: &str| fixtures.join(name);
        let size = |name: &str| std::fs::metadata(file(name)).unwrap().len();
        let chosen = Chosen {
            tracks: vec![
                Track {
                    path: file("sine.flac").to_string_lossy().into(),
                    file_size: size("sine.flac"),
                    title: "Sine".into(),
                    ..track(1, "FLAC", MediaKind::Music)
                },
                Track {
                    path: file("sine.mp3").to_string_lossy().into(),
                    file_size: size("sine.mp3"),
                    title: "Beep".into(),
                    ..track(2, "MP3", MediaKind::Music)
                },
            ],
            playlists: vec![("Tones".into(), vec![2, 1])],
            in_cloud: 0,
            missing: 0,
        };
        let mut stages = Vec::new();
        let summary = sync_to_player(&player, &chosen, work.path(), &AtomicBool::new(false), &mut |p| stages.push(p.stage)).unwrap();
        assert_eq!((summary.copied, summary.skipped.len()), (2, 0));

        let mut files: Vec<PathBuf> = walk(root.path()).into_iter().filter(|p| !p.to_string_lossy().contains(".unsub")).collect();
        files.sort();
        let names: Vec<String> = files.iter().map(|p| p.strip_prefix(root.path()).unwrap().to_string_lossy().into_owned()).collect();
        assert!(names.iter().any(|n| n.ends_with("Sine.mp3")), "{names:?}");
        assert!(names.iter().any(|n| n.ends_with("Beep.mp3")), "{names:?}");
        assert!(names.iter().all(|n| !n.ends_with(".flac")), "{names:?}");
        let copied = files.iter().find(|p| p.to_string_lossy().ends_with("Beep.mp3")).unwrap();
        assert_eq!(std::fs::read(copied).unwrap(), std::fs::read(file("sine.mp3")).unwrap());
        let playlist = files.iter().find(|p| p.extension().is_some_and(|e| e == "m3u8")).expect("a playlist");
        let text = std::fs::read_to_string(playlist).unwrap();
        assert!(text.find("Beep").unwrap() < text.find("Sine").unwrap(), "{text}");
        assert!(walk(work.path()).is_empty(), "converted files left behind");
        assert!(stages.contains(&unsub_devicesync::SyncStage::Copying));
    }

    fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.extend(walk(&path));
            } else {
                found.push(path);
            }
        }
        found
    }
}
