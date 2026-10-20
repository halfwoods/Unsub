use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use unsub_ipod::{Ipod, IpodMediaKind, IpodModel, IpodPlaylist, IpodTrack, SyncSupport, Usage};
use unsub_library::{Library, LibraryStats, MediaKind, Track, TrackQuery};

use crate::cd::Cds;
use crate::error::{CommandError, CommandResult};
use crate::playback::{PlaySource, PlaybackState};

pub const CHANGED_EVENT: &str = "devices://changed";

/// iPods mount like any USB disk; polling the mount roots is portable and cheap enough at this rate.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// A connected iPod. Its tracks live in an in-memory library so device lists sort and search
/// exactly like the main library.
struct Connected {
    ipod: Ipod,
    playlists: Vec<IpodPlaylist>,
    usage: Usage,
    counts: KindCounts,
    tracks: Mutex<Library>,
}

impl Connected {
    fn tracks(&self) -> MutexGuard<'_, Library> {
        self.tracks.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Default, Clone, Copy, Serialize)]
struct KindCounts {
    music: u32,
    podcast: u32,
    audiobook: u32,
}

#[derive(Default)]
pub struct DevicesState {
    /// Keyed by mount path, which is what discovery reports.
    connected: Mutex<HashMap<PathBuf, Arc<Connected>>>,
    pub cds: Cds,
    /// Non-Apple players: Rockbox, marked USB players and phones.
    players: Mutex<Vec<unsub_devicesync::Device>>,
}

impl DevicesState {
    fn connected(&self) -> MutexGuard<'_, HashMap<PathBuf, Arc<Connected>>> {
        self.connected.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn find(&self, device_id: &str) -> CommandResult<Arc<Connected>> {
        self.connected()
            .values()
            .find(|device| device.ipod.id == device_id)
            .cloned()
            .ok_or_else(|| CommandError::from(format!("device {device_id} is not connected")))
    }

    fn players(&self) -> MutexGuard<'_, Vec<unsub_devicesync::Device>> {
        self.players.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn player(&self, device_id: &str) -> Option<unsub_devicesync::Device> {
        self.players().iter().find(|p| p.id == device_id).cloned()
    }

    /// Looks for players again, e.g. after a sync changed one's free space.
    pub(crate) fn rediscover_players(&self) {
        *self.players() = unsub_devicesync::discover();
    }

    /// The iPod behind a device id, for syncing.
    pub(crate) fn ipod(&self, device_id: &str) -> CommandResult<Ipod> {
        Ok(self.find(device_id)?.ipod.clone())
    }

    /// Re-reads an iPod whose database a sync just rewrote.
    pub(crate) fn reconnect(&self, mount: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        let device = connect(mount)?;
        self.connected().insert(mount.to_path_buf(), Arc::new(device));
        Ok(())
    }

    pub fn track_path(&self, device_id: &str, track_id: i64) -> CommandResult<PathBuf> {
        self.find(device_id)?
            .tracks()
            .track(track_id)?
            .map(|track| PathBuf::from(track.path))
            .ok_or_else(|| CommandError::from(format!("track {track_id} is not on device {device_id}")))
    }

    pub fn artwork(&self, device_id: &str, track_id: i64) -> Option<unsub_library::Artwork> {
        self.find(device_id).ok()?.tracks().artwork(track_id).ok().flatten()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    id: String,
    name: String,
    model: Option<IpodModel>,
    capacity_bytes: u64,
    free_bytes: u64,
    serial_number: Option<String>,
    sync: SyncSupport,
    usage: Usage,
    counts: KindCounts,
    playlists: Vec<PlaylistSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistSummary {
    /// iTunesDB ids are 64-bit; as JSON numbers they'd lose precision in JavaScript.
    id: String,
    name: String,
    is_podcasts: bool,
    is_smart: bool,
    track_count: usize,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum DeviceView {
    Kind {
        kind: MediaKind,
    },
    Playlist {
        #[serde(rename = "playlistId")]
        playlist_id: String,
    },
}

fn media_kind(kind: &IpodMediaKind) -> Option<MediaKind> {
    match kind {
        IpodMediaKind::Music => Some(MediaKind::Music),
        IpodMediaKind::Podcast => Some(MediaKind::Podcast),
        IpodMediaKind::Audiobook => Some(MediaKind::Audiobook),
        // Unsub is audio-only; video still counts toward the capacity bar.
        IpodMediaKind::Video | IpodMediaKind::Other => None,
    }
}

fn to_track(track: &IpodTrack, kind: MediaKind) -> Track {
    Track {
        id: i64::from(track.id),
        path: track.path.to_string_lossy().into_owned(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album_artist: track.album_artist.clone(),
        album: track.album.clone(),
        genre: track.genre.clone(),
        composer: track.composer.clone(),
        year: track.year,
        track_number: track.track_number,
        track_count: track.track_count,
        disc_number: track.disc_number,
        disc_count: track.disc_count,
        duration_ms: track.duration_ms,
        bitrate_kbps: track.bitrate_kbps,
        sample_rate: track.sample_rate,
        bit_depth: None,
        channels: None,
        codec: track.file_type.clone(),
        file_size: track.file_size,
        date_added: track.date_added.unwrap_or(0),
        date_modified: track.date_added.unwrap_or(0),
        play_count: track.play_count,
        last_played: track.last_played,
        skip_count: track.skip_count,
        rating: track.rating,
        kind,
        // iPods have no checkboxes; every song on one is in play.
        enabled: true,
        // Songs on an iPod are on the iPod's own disk, never in iCloud or missing from the library.
        in_cloud: false,
        missing: false,
        loudness: None,
        remember_position: false,
        has_lyrics: false,
        compilation: track.compilation,
        sort_title: track.sort_title.clone(),
        sort_artist: track.sort_artist.clone(),
        sort_album: track.sort_album.clone(),
        sort_album_artist: track.sort_album_artist.clone(),
    }
}

fn connect(mount: &std::path::Path) -> Result<Connected, Box<dyn std::error::Error>> {
    let ipod = unsub_ipod::open(mount)?;
    let database = unsub_ipod::read_database(&ipod)?;
    let mut counts = KindCounts::default();
    let tracks: Vec<Track> = database
        .tracks
        .iter()
        .filter_map(|track| {
            let kind = media_kind(&track.media_kind)?;
            match kind {
                MediaKind::Music => counts.music += 1,
                MediaKind::Podcast => counts.podcast += 1,
                MediaKind::Audiobook => counts.audiobook += 1,
                // iPods hold no radio stations; media_kind never yields one.
                MediaKind::Radio => {}
            }
            Some(to_track(track, kind))
        })
        .collect();
    let mut library = Library::open_in_memory()?;
    library.import_tracks(&tracks)?;
    Ok(Connected { ipod, playlists: database.playlists, usage: database.usage, counts, tracks: Mutex::new(library) })
}

/// Watches for iPods arriving and leaving for the life of the app.
pub fn watch(app: AppHandle) {
    std::thread::spawn(move || {
        loop {
            let players = unsub_devicesync::discover();
            let player_mounts: Vec<PathBuf> = players
                .iter()
                .filter_map(|p| match &p.root {
                    unsub_devicesync::DeviceRoot::Path(path) => Some(path.clone()),
                    unsub_devicesync::DeviceRoot::Mtp { .. } => None,
                })
                .collect();
            // A Rockbox-flashed iPod still has iPod_Control; it syncs as the player it now is.
            let mounts: Vec<PathBuf> = unsub_ipod::discover().into_iter().filter(|m| !player_mounts.contains(m)).collect();
            let state = app.state::<DevicesState>();
            let mut changed = {
                let mut known = state.players();
                let before: Vec<&str> = known.iter().map(|p| p.id.as_str()).collect();
                let arrived_or_left = before != players.iter().map(|p| p.id.as_str()).collect::<Vec<_>>();
                *known = players;
                arrived_or_left
            };
            let known: Vec<PathBuf> = state.connected().keys().cloned().collect();

            for gone in known.iter().filter(|mount| !mounts.contains(mount)) {
                // Bind first so the map lock is released before playback's lock is taken (lock order).
                let removed = state.connected().remove(gone);
                if let Some(device) = removed {
                    app.state::<PlaybackState>().stop_if_playing(&PlaySource::Device { device_id: device.ipod.id.clone() });
                    changed = true;
                }
            }
            // Reading a big iTunesDB over USB takes a moment, so it happens outside the map lock.
            for mount in mounts.iter().filter(|mount| !known.contains(mount)) {
                match connect(mount) {
                    Ok(device) => {
                        state.connected().insert(mount.clone(), Arc::new(device));
                        changed = true;
                    }
                    Err(error) => eprintln!("[devices] could not read iPod at {}: {error}", mount.display()),
                }
            }
            if changed {
                let _ = app.emit(CHANGED_EVENT, ());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    });
}

#[tauri::command]
pub fn players_list(state: State<'_, DevicesState>) -> Vec<unsub_devicesync::Device> {
    state.players().clone()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayerPreset {
    MassStorage,
    Android,
    Rockbox,
}

#[tauri::command]
pub fn player_eject(app: AppHandle, state: State<'_, DevicesState>, device_id: String) -> CommandResult<()> {
    let player = state.player(&device_id).ok_or_else(|| CommandError::from("that player is no longer connected".to_owned()))?;
    unsub_devicesync::eject(&player).map_err(|error| CommandError::from(error.to_string()))?;
    state.rediscover_players();
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(())
}

/// Makes a drive a sync target by writing devicesync's marker file to it, which holds its name
/// and what it plays.
#[tauri::command]
pub fn player_set_up(
    app: AppHandle,
    state: State<'_, DevicesState>,
    path: PathBuf,
    name: String,
    preset: PlayerPreset,
) -> CommandResult<()> {
    let profile = match preset {
        PlayerPreset::MassStorage => unsub_devicesync::Profile::mass_storage(),
        PlayerPreset::Android => unsub_devicesync::Profile::android(),
        PlayerPreset::Rockbox => unsub_devicesync::Profile::rockbox(),
    };
    unsub_devicesync::mark_device(&path, &name, &profile).map_err(|error| CommandError::from(error.to_string()))?;
    state.rediscover_players();
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(())
}

#[tauri::command]
pub fn devices_list(state: State<'_, DevicesState>) -> Vec<DeviceSummary> {
    let mut devices: Vec<DeviceSummary> = state
        .connected()
        .values()
        .map(|device| DeviceSummary {
            id: device.ipod.id.clone(),
            name: device.ipod.name.clone(),
            model: device.ipod.model.clone(),
            capacity_bytes: device.ipod.capacity_bytes,
            free_bytes: device.ipod.free_bytes,
            serial_number: device.ipod.serial_number.clone(),
            sync: device.ipod.sync,
            usage: device.usage,
            counts: device.counts,
            // The master playlist is the device itself in the sidebar.
            playlists: device
                .playlists
                .iter()
                .filter(|playlist| !playlist.is_master)
                .map(|playlist| PlaylistSummary {
                    id: playlist.id.to_string(),
                    name: playlist.name.clone(),
                    is_podcasts: playlist.is_podcasts,
                    is_smart: playlist.is_smart,
                    track_count: playlist.track_ids.len(),
                })
                .collect(),
        })
        .collect();
    devices.sort_by(|a, b| a.name.cmp(&b.name));
    devices
}

fn scoped(device: &Connected, view: DeviceView, mut query: TrackQuery) -> CommandResult<TrackQuery> {
    match view {
        DeviceView::Kind { kind } => query.kind = Some(kind),
        DeviceView::Playlist { playlist_id } => {
            let playlist = device
                .playlists
                .iter()
                .find(|playlist| playlist.id.to_string() == playlist_id)
                .ok_or_else(|| CommandError::from(format!("playlist {playlist_id} is not on this device")))?;
            query.kind = None;
            query.ids = Some(playlist.track_ids.iter().map(|&id| i64::from(id)).collect());
        }
    }
    Ok(query)
}

#[tauri::command]
pub fn device_tracks(state: State<'_, DevicesState>, device_id: String, view: DeviceView, query: TrackQuery) -> CommandResult<Vec<Track>> {
    let device = state.find(&device_id)?;
    let query = scoped(&device, view, query)?;
    Ok(device.tracks().tracks(&query)?)
}

#[tauri::command]
pub fn device_stats(state: State<'_, DevicesState>, device_id: String, view: DeviceView, query: TrackQuery) -> CommandResult<LibraryStats> {
    let device = state.find(&device_id)?;
    let query = scoped(&device, view, query)?;
    Ok(device.tracks().stats(&query)?)
}

#[tauri::command]
pub fn device_eject(app: AppHandle, state: State<'_, DevicesState>, device_id: String) -> CommandResult<()> {
    let device = state.find(&device_id)?;
    // Release our open files first, or the OS refuses to unmount.
    app.state::<PlaybackState>().stop_if_playing(&PlaySource::Device { device_id: device_id.clone() });
    unsub_ipod::eject(&device.ipod).map_err(|error| CommandError::from(error.to_string()))?;
    state.connected().retain(|_, connected| connected.ipod.id != device_id);
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(())
}
