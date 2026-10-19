use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use antistreaming_playback::{PlaybackState as EngineState, PlaybackStatus, Player, PlayerEvent};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::devices::DevicesState;
use crate::error::{CommandError, CommandResult};
use crate::library::LibraryState;
use crate::party::{self, PartyShuffle};
use crate::podcasts::PodcastsState;
use crate::queue::{PlayQueue, RepeatMode};

pub const PLAYER_EVENT: &str = "player://event";

/// iTunes restarts the current song instead of going back once you're this far in.
const PREVIOUS_RESTARTS_AFTER_MS: u64 = 3_000;
/// iTunes counts a skip only when you bail early, but not instantly.
const SKIP_WINDOW_MS: std::ops::Range<u64> = 2_000..20_000;

/// Where queued track ids live. Library and iPod ids overlap, so the queue must remember.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PlaySource {
    Library,
    Device {
        #[serde(rename = "deviceId")]
        device_id: String,
    },
}

pub(crate) struct Playing {
    pub(crate) source: PlaySource,
    pub(crate) queue: PlayQueue,
    /// Party Shuffle owns what comes next; the queue only ever holds its current and upcoming songs.
    pub(crate) party: bool,
}

struct Modes {
    shuffle: bool,
    repeat: RepeatMode,
}

/// Resolves queued ids to files, whichever source they came from.
pub(crate) struct Tracks<'a> {
    pub(crate) library: &'a LibraryState,
    pub(crate) devices: &'a DevicesState,
}

impl Tracks<'_> {
    fn path(&self, source: &PlaySource, track_id: i64) -> CommandResult<PathBuf> {
        match source {
            PlaySource::Library => self.library.track_path(track_id),
            PlaySource::Device { device_id } => self.devices.track_path(device_id, track_id),
        }
    }
}

/// Lock order is always playing → party → library/devices, from commands and engine events alike.
pub struct PlaybackState {
    pub(crate) player: Player,
    playing: Mutex<Option<Playing>>,
    modes: Mutex<Modes>,
    party: Mutex<PartyShuffle>,
}

impl PlaybackState {
    pub fn new(player: Player) -> Self {
        Self {
            player,
            playing: Mutex::new(None),
            modes: Mutex::new(Modes { shuffle: false, repeat: RepeatMode::Off }),
            party: Mutex::new(PartyShuffle::default()),
        }
    }

    pub(crate) fn party(&self) -> MutexGuard<'_, PartyShuffle> {
        self.party.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn playing(&self) -> MutexGuard<'_, Option<Playing>> {
        self.playing.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn modes(&self) -> MutexGuard<'_, Modes> {
        self.modes.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn start(&self, tracks: &Tracks, playing: &Playing) -> CommandResult<()> {
        let track_id = playing.queue.current();
        self.player.play(track_id, tracks.path(&playing.source, track_id)?);
        self.queue_following(tracks, playing)
    }

    /// Hands the engine the next track early so it can play gaplessly.
    pub(crate) fn queue_following(&self, tracks: &Tracks, playing: &Playing) -> CommandResult<()> {
        match playing.queue.peek_next() {
            Some(next) => self.player.queue_next(next, tracks.path(&playing.source, next)?),
            None => self.player.clear_next(),
        }
        Ok(())
    }

    /// Stops and forgets the queue when its source goes away, e.g. an iPod is unplugged.
    pub fn stop_if_playing(&self, source: &PlaySource) {
        let mut slot = self.playing();
        if slot.as_ref().is_some_and(|playing| &playing.source == source) {
            self.player.stop();
            *slot = None;
        }
    }
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The engine stops decoding while its callback runs, so the callback only enqueues. This thread
/// does the work that can wait on SQLite or the webview, in the order events happened.
pub fn spawn_event_worker(app: AppHandle) -> std::io::Result<Sender<PlayerEvent>> {
    let (events, received) = mpsc::channel();
    std::thread::Builder::new()
        .name("antistreaming-player-events".into())
        .spawn(move || {
            for event in received {
                handle_event(&app, event);
            }
        })?;
    Ok(events)
}

/// Forwards every event to the UI and keeps the queue in step.
fn handle_event(app: &AppHandle, event: PlayerEvent) {
    let _ = app.emit(PLAYER_EVENT, &event);
    let (Some(playback), Some(library), Some(devices)) = (
        app.try_state::<PlaybackState>(),
        app.try_state::<LibraryState>(),
        app.try_state::<DevicesState>(),
    ) else {
        return;
    };
    let tracks = Tracks { library: &library, devices: &devices };
    let mut slot = playback.playing();
    let Some(playing) = slot.as_mut() else { return };
    let result = match event {
        PlayerEvent::TrackStarted { track_id } if playing.party => party::on_track_started(app, &playback, &tracks, playing, track_id),
        PlayerEvent::TrackStarted { track_id } if playing.queue.sync_to(track_id) => {
            // Podcasts and audiobooks pick up where the listener stopped, as in iTunes.
            if let Some(position) = app.try_state::<PodcastsState>().and_then(|p| p.resume_position(track_id)) {
                playback.player.seek(position);
            }
            playback.queue_following(&tracks, playing)
        }
        // A play counts when the song reaches its end, as in iTunes. iPod counts sync back in milestone 13.
        PlayerEvent::TrackEnded { track_id } if playing.source == PlaySource::Library => {
            if let Some(podcasts) = app.try_state::<PodcastsState>() {
                podcasts.track_finished(track_id);
            }
            library.queries().record_play(track_id, now_unix()).map_err(CommandError::from)
        }
        PlayerEvent::Position { track_id, position_ms, .. } if playing.source == PlaySource::Library => {
            if let Some(podcasts) = app.try_state::<PodcastsState>() {
                podcasts.track_position(track_id, position_ms);
            }
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(error) = result {
        eprintln!("[playback] {error:?}");
    }
}

#[tauri::command]
pub fn player_play_list(
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
    source: PlaySource,
    track_ids: Vec<i64>,
    start_index: usize,
) -> CommandResult<()> {
    let (shuffle, repeat) = {
        let modes = playback.modes();
        (modes.shuffle, modes.repeat)
    };
    let queue = PlayQueue::new(track_ids, start_index, shuffle, repeat)
        .ok_or_else(|| CommandError::from(format!("no track at index {start_index}")))?;
    let playing = Playing { source, queue, party: false };
    let mut slot = playback.playing();
    playback.start(&Tracks { library: &library, devices: &devices }, &playing)?;
    *slot = Some(playing);
    Ok(())
}

#[tauri::command]
pub fn player_toggle_pause(
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
) -> CommandResult<()> {
    match playback.player.status().state {
        EngineState::Playing => playback.player.pause(),
        EngineState::Paused => playback.player.resume(),
        EngineState::Stopped => {
            if let Some(playing) = playback.playing().as_ref() {
                playback.start(&Tracks { library: &library, devices: &devices }, playing)?;
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn player_next(
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
) -> CommandResult<()> {
    let position_ms = playback.player.status().position_ms;
    let mut slot = playback.playing();
    let Some(playing) = slot.as_mut() else { return Ok(()) };
    if playing.source == PlaySource::Library && SKIP_WINDOW_MS.contains(&position_ms) {
        library.queries().record_skip(playing.queue.current(), now_unix())?;
    }
    if playing.queue.skip_next().is_some() {
        playback.start(&Tracks { library: &library, devices: &devices }, playing)
    } else {
        playback.player.stop();
        *slot = None;
        Ok(())
    }
}

#[tauri::command]
pub fn player_previous(
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
) -> CommandResult<()> {
    let mut slot = playback.playing();
    let Some(playing) = slot.as_mut() else { return Ok(()) };
    if playback.player.status().position_ms > PREVIOUS_RESTARTS_AFTER_MS || playing.queue.skip_previous().is_none() {
        playback.player.seek(0);
        return Ok(());
    }
    playback.start(&Tracks { library: &library, devices: &devices }, playing)
}

#[tauri::command]
pub fn player_seek(playback: State<'_, PlaybackState>, position_ms: u64) {
    playback.player.seek(position_ms);
}

#[tauri::command]
pub fn player_set_volume(playback: State<'_, PlaybackState>, volume: f32) {
    playback.player.set_volume(volume.clamp(0.0, 1.0));
}

#[tauri::command]
pub fn player_set_shuffle(
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
    shuffle: bool,
) -> CommandResult<()> {
    playback.modes().shuffle = shuffle;
    let mut slot = playback.playing();
    let Some(playing) = slot.as_mut() else { return Ok(()) };
    playing.queue.set_shuffle(shuffle);
    playback.queue_following(&Tracks { library: &library, devices: &devices }, playing)
}

#[tauri::command]
pub fn player_set_repeat(
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
    repeat: RepeatMode,
) -> CommandResult<()> {
    playback.modes().repeat = repeat;
    let mut slot = playback.playing();
    let Some(playing) = slot.as_mut() else { return Ok(()) };
    playing.queue.set_repeat(repeat);
    playback.queue_following(&Tracks { library: &library, devices: &devices }, playing)
}

#[tauri::command]
pub fn player_set_bit_perfect(playback: State<'_, PlaybackState>, enabled: bool) {
    playback.player.set_bit_perfect(enabled);
}

#[tauri::command]
pub fn player_status(playback: State<'_, PlaybackState>) -> PlaybackStatus {
    playback.player.status()
}
