use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use unsub_library::{Library, MediaKind, PlaylistKind, SortKey, Track, TrackQuery};

use crate::devices::DevicesState;
use crate::error::{CommandError, CommandResult};
use crate::jobs::{self, Ctx, Job, Kind};
use crate::library::LibraryState;

mod cloud;
mod ipod;
mod players;

use ipod::{Conversion, for_ipod, fresh_ipod};
use players::{for_player, player_target};

pub use cloud::{forget_leftover, tidy};

/// Progress while an iPod or player syncs; `null` once it's done.
pub const SYNC_EVENT: &str = "devices://sync";

/// What lossy files the iPod can't play become, when no conversion is chosen.
const FALLBACK_AAC_KBPS: u32 = 256;
const LOSSY_CODECS: [&str; 4] = ["AAC", "MP3", "Vorbis", "Opus"];

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn to_error(error: impl std::fmt::Display) -> CommandError {
    CommandError::from(error.to_string())
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MusicScope {
    #[default]
    All,
    Playlists {
        ids: Vec<i64>,
    },
}

/// iTunes' per-iPod choices from its Summary, Music, Podcasts and Books tabs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncSettings {
    music: bool,
    music_scope: MusicScope,
    podcasts: bool,
    podcasts_unplayed_only: bool,
    audiobooks: bool,
    only_checked: bool,
    /// iTunes' "Convert higher bit rate songs to … AAC"; `None` keeps lossless as Apple Lossless.
    convert_kbps: Option<u32>,
    /// The writer is new, so the first sync to each iPod asks before touching it.
    confirmed: bool,
    /// Songs that are only in iCloud come down one at a time while an iPod syncs; this gives each
    /// back to iCloud once it is on the iPod, so the library never has to fit on the Mac.
    remove_downloaded: bool,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            music: true,
            music_scope: MusicScope::All,
            podcasts: true,
            podcasts_unplayed_only: true,
            audiobooks: true,
            only_checked: false,
            convert_kbps: None,
            confirmed: false,
            remove_downloaded: true,
        }
    }
}

/// Sync settings per iPod, kept in DeviceSync.json.
pub struct SyncState {
    path: PathBuf,
    settings: Mutex<HashMap<String, SyncSettings>>,
}

impl SyncState {
    pub fn load(path: PathBuf) -> Self {
        let settings = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { path, settings: Mutex::new(settings) }
    }

    fn settings_for(&self, device_id: &str) -> SyncSettings {
        lock(&self.settings).get(device_id).cloned().unwrap_or_default()
    }

    fn save(&self, device_id: &str, settings: SyncSettings) -> CommandResult<()> {
        let mut all = lock(&self.settings);
        all.insert(device_id.to_owned(), settings);
        let json = serde_json::to_vec_pretty(&*all).map_err(to_error)?;
        std::fs::write(&self.path, json).map_err(to_error)
    }
}

/// The library songs and playlists a sync takes, whatever the device, and what it left out.
struct Chosen {
    tracks: Vec<Track>,
    /// Each playlist's name and its songs in order, limited to the chosen songs.
    playlists: Vec<(String, Vec<i64>)>,
    /// Songs left out because they are only in iCloud, for a device that can't fetch them.
    in_cloud: usize,
    missing: usize,
}

fn is_lossless(track: &Track) -> bool {
    !LOSSY_CODECS.contains(&track.codec.as_str())
}

fn query(kind: Option<MediaKind>, playlist: Option<i64>) -> TrackQuery {
    TrackQuery { kind, playlist, sort: if playlist.is_some() { SortKey::PlaylistOrder } else { SortKey::Artist }, ..TrackQuery::default() }
}

/// `fetch_cloud` keeps the songs that are only in iCloud, for a sync that downloads them as it goes.
fn select(library: &Library, settings: &SyncSettings, fetch_cloud: bool) -> unsub_library::Result<Chosen> {
    let all_playlists = library.playlists()?;
    let synced_playlists: Vec<_> = match &settings.music_scope {
        // Syncing everything brings every playlist along, as iTunes did.
        MusicScope::All => all_playlists.iter().filter(|p| p.kind != PlaylistKind::Folder).collect(),
        MusicScope::Playlists { ids } => all_playlists.iter().filter(|p| ids.contains(&p.id)).collect(),
    };

    let mut candidates: Vec<Track> = Vec::new();
    if settings.music {
        match settings.music_scope {
            MusicScope::All => candidates.extend(library.tracks(&query(Some(MediaKind::Music), None))?),
            MusicScope::Playlists { .. } => {
                for playlist in &synced_playlists {
                    candidates.extend(library.tracks(&query(None, Some(playlist.id)))?);
                }
            }
        }
    }
    if settings.podcasts {
        let episodes = library.tracks(&query(Some(MediaKind::Podcast), None))?;
        candidates.extend(episodes.into_iter().filter(|t| !settings.podcasts_unplayed_only || t.play_count == 0));
    }
    if settings.audiobooks {
        candidates.extend(library.tracks(&query(Some(MediaKind::Audiobook), None))?);
    }

    let mut chosen = Chosen { tracks: Vec::new(), playlists: Vec::new(), in_cloud: 0, missing: 0 };
    let mut seen = HashSet::new();
    for track in candidates {
        // A playlist can hold radio stations, which no device can carry.
        if track.kind == MediaKind::Radio || !seen.insert(track.id) || (settings.only_checked && !track.enabled) {
            continue;
        }
        if track.missing {
            chosen.missing += 1;
            continue;
        }
        if track.in_cloud && !fetch_cloud {
            chosen.in_cloud += 1;
            continue;
        }
        chosen.tracks.push(track);
    }
    if settings.music {
        let ids: HashSet<i64> = chosen.tracks.iter().map(|t| t.id).collect();
        for playlist in synced_playlists {
            let tracks: Vec<i64> =
                library.tracks(&query(None, Some(playlist.id)))?.into_iter().map(|t| t.id).filter(|id| ids.contains(id)).collect();
            chosen.playlists.push((playlist.name.clone(), tracks));
        }
    }
    Ok(chosen)
}

#[tauri::command]
pub fn device_sync_settings(state: State<'_, SyncState>, device_id: String) -> SyncSettings {
    state.settings_for(&device_id)
}

#[tauri::command]
pub fn device_set_sync_settings(state: State<'_, SyncState>, device_id: String, settings: SyncSettings) -> CommandResult<()> {
    state.save(&device_id, settings)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPreview {
    tracks: usize,
    add: usize,
    update: usize,
    remove: usize,
    bytes_needed: u64,
    free_bytes: u64,
    fits: bool,
    converted: usize,
    /// Songs the sync will fetch from iCloud, which only an iPod does.
    download: usize,
    /// Songs left out because they are only in iCloud.
    in_cloud: usize,
    missing: usize,
}

/// What Sync would do with the current settings; touches nothing.
#[tauri::command]
pub async fn device_sync_preview(app: AppHandle, device_id: String) -> CommandResult<SyncPreview> {
    let settings = app.state::<SyncState>().settings_for(&device_id);
    let player = app.state::<DevicesState>().player(&device_id);
    let chosen = select(&app.state::<LibraryState>().queries(), &settings, player.is_none())?;
    if let Some(player) = player {
        let (items, playlists) = for_player(&chosen);
        let plan = unsub_devicesync::plan(&player, &items, &playlists).map_err(to_error)?;
        let converting: HashSet<i64> = chosen.tracks.iter().filter(|t| player_target(t, &player.profile).is_some()).map(|t| t.id).collect();
        return Ok(SyncPreview {
            tracks: items.len(),
            add: plan.add.len(),
            update: plan.update.len(),
            remove: plan.remove.len(),
            bytes_needed: plan.bytes_needed,
            free_bytes: plan.free_bytes,
            fits: plan.fits,
            converted: plan.add.iter().chain(&plan.update).filter(|f| converting.contains(&f.library_id)).count(),
            download: 0,
            in_cloud: chosen.in_cloud,
            missing: chosen.missing,
        });
    }
    let ipod = fresh_ipod(&app.state::<DevicesState>(), &device_id)?;
    let selection = for_ipod(&chosen, settings.convert_kbps);
    let database = unsub_ipod::read_database(&ipod).map_err(to_error)?;
    let plan = unsub_ipod::plan_sync(&ipod, &database, &selection.tracks, &selection.playlists).map_err(to_error)?;
    Ok(SyncPreview {
        tracks: selection.tracks.len(),
        add: plan.add.len(),
        update: plan.update.len(),
        remove: plan.remove.len(),
        bytes_needed: plan.bytes_needed,
        free_bytes: ipod.free_bytes,
        fits: plan.fits,
        converted: plan.add.iter().filter(|id| selection.conversions.get(id).is_some_and(|(c, _)| *c != Conversion::Copy)).count(),
        download: plan.add.iter().filter(|id| selection.in_cloud.contains(id)).count(),
        in_cloud: chosen.in_cloud,
        missing: chosen.missing,
    })
}

const WORK_DIR: &str = "Device Sync";

/// Where a sync's converted songs are made; it never holds more than one at a time.
fn fresh_work_dir(app: &AppHandle) -> CommandResult<PathBuf> {
    let work_dir = app.path().app_cache_dir().map_err(to_error)?.join(WORK_DIR);
    // Clears what an interrupted sync may have left.
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir).map_err(to_error)?;
    Ok(work_dir)
}

fn run_sync(app: &AppHandle, ctx: &Ctx, device_id: &str) -> CommandResult<()> {
    match app.state::<DevicesState>().player(device_id) {
        Some(player) => players::run_player_sync(app, ctx, &player),
        None => ipod::run_ipod_sync(app, ctx, device_id),
    }
}

#[tauri::command]
pub fn device_sync(app: AppHandle, device_id: String) -> CommandResult<()> {
    let job = Job::new(Kind::Sync, SYNC_EVENT).resource(&device_id);
    jobs::start(&app, job, move |app, ctx| {
        if let Err(error) = run_sync(app, ctx, &device_id) {
            ctx.notice(if ctx.cancelled() { "Sync stopped".to_owned() } else { format!("Sync failed: {error}") });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::track;

    fn library() -> (Library, i64) {
        let mut library = Library::open_in_memory().unwrap();
        library
            .import_tracks(&[
                track(1, "FLAC", MediaKind::Music),
                Track { enabled: false, ..track(2, "MP3", MediaKind::Music) },
                Track { in_cloud: true, ..track(3, "AAC", MediaKind::Music) },
                Track { missing: true, ..track(4, "AAC", MediaKind::Music) },
                Track { play_count: 2, ..track(5, "MP3", MediaKind::Podcast) },
                track(6, "MP3", MediaKind::Podcast),
                track(7, "AAC", MediaKind::Audiobook),
            ])
            .unwrap();
        let playlist = library.create_playlist("Road Trip", PlaylistKind::Regular, None).unwrap().id;
        library.add_to_playlist(playlist, &[2, 1, 3], None).unwrap();
        (library, playlist)
    }

    fn ids(chosen: &Chosen) -> Vec<i64> {
        let mut ids: Vec<i64> = chosen.tracks.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn the_whole_library_syncs_with_its_playlists_and_skips_what_it_cannot_copy() {
        let (library, _) = library();
        let selection = select(&library, &SyncSettings::default(), false).unwrap();
        assert_eq!(ids(&selection), [1, 2, 6, 7]);
        assert_eq!((selection.in_cloud, selection.missing), (1, 1));
        // Every playlist comes along, the default smart ones included, as in iTunes; each keeps
        // its order but lists only songs that are going onto the iPod.
        let names: Vec<&str> = selection.playlists.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"Top 25 Most Played"));
        let (_, road_trip) = selection.playlists.iter().find(|(name, _)| name == "Road Trip").unwrap();
        assert_eq!(road_trip, &[2, 1]);
    }

    #[test]
    fn a_device_that_fetches_from_icloud_keeps_those_songs_in_the_sync_and_its_playlists() {
        let (library, _) = library();
        let selection = select(&library, &SyncSettings::default(), true).unwrap();
        assert_eq!(ids(&selection), [1, 2, 3, 6, 7]);
        assert_eq!((selection.in_cloud, selection.missing), (0, 1));
        let (_, road_trip) = selection.playlists.iter().find(|(name, _)| name == "Road Trip").unwrap();
        assert_eq!(road_trip, &[2, 1, 3]);
        // The iPod is told which of them are only in iCloud, so the preview counts the ones it adds.
        let ipod = for_ipod(&selection, None);
        assert_eq!(ipod.in_cloud, HashSet::from([3]));
    }

    #[test]
    fn songs_are_given_back_to_icloud_unless_the_setting_is_turned_off() {
        assert!(SyncSettings::default().remove_downloaded);
        // Settings saved before the option existed read as having it on.
        let old: SyncSettings = serde_json::from_str(r#"{"music":false,"convertKbps":128}"#).unwrap();
        assert!(old.remove_downloaded && !old.music && old.convert_kbps == Some(128));
        let off: SyncSettings = serde_json::from_str(r#"{"removeDownloaded":false}"#).unwrap();
        assert!(!off.remove_downloaded);
    }

    #[test]
    fn only_checked_songs_and_chosen_playlists_narrow_the_sync() {
        let (library, playlist) = library();
        let checked = SyncSettings { only_checked: true, ..SyncSettings::default() };
        assert_eq!(ids(&select(&library, &checked, false).unwrap()), [1, 6, 7]);

        let chosen = SyncSettings {
            music_scope: MusicScope::Playlists { ids: vec![playlist] },
            podcasts: false,
            audiobooks: false,
            ..SyncSettings::default()
        };
        let selection = select(&library, &chosen, false).unwrap();
        assert_eq!(ids(&selection), [1, 2]);
        assert_eq!(selection.playlists[0].1, [2, 1]);
    }

    #[test]
    fn played_podcast_episodes_stay_behind_unless_asked_for() {
        let (library, _) = library();
        let music_off = SyncSettings { music: false, audiobooks: false, ..SyncSettings::default() };
        assert_eq!(ids(&select(&library, &music_off, false).unwrap()), [6]);
        let every_episode = SyncSettings { podcasts_unplayed_only: false, ..music_off };
        assert_eq!(ids(&select(&library, &every_episode, false).unwrap()), [5, 6]);
    }
}
