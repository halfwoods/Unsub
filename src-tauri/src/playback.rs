use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use antistreaming_library::GainMode;
use antistreaming_playback::{Gain, PlaybackState as EngineState, PlaybackStatus, Player, PlayerEvent};
use antistreaming_remote::Client;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::devices::DevicesState;
use crate::error::{CommandError, CommandResult};
use crate::library::LibraryState;
use crate::party::{self, PartyShuffle};
use crate::podcasts::PodcastsState;
use crate::queue::{PlayQueue, RepeatMode};
use crate::remote::RemoteState;

pub const PLAYER_EVENT: &str = "player://event";
/// Sound Check's target, about where iTunes levelled songs.
const SOUND_CHECK_LUFS: f64 = -16.0;

/// Progress while a song downloads (from iCloud Drive or a shared library) before it can play;
/// `null` when done.
pub const DOWNLOADING_EVENT: &str = "player://downloading";

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Downloading {
    track_id: i64,
    done: u64,
    total: u64,
    /// "iCloud", or the shared library's name.
    from: String,
}

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
    /// An audio CD, whose track ids are its track numbers.
    Cd {
        #[serde(rename = "discId")]
        disc_id: String,
    },
    /// A shared library, whose track ids are positions in its song list.
    Shared {
        library: String,
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
            PlaySource::Cd { disc_id } => self.devices.cds.track_path(disc_id, track_id),
            PlaySource::Shared { .. } => Err(CommandError::from("shared songs are fetched, not opened".to_owned())),
        }
    }
}

/// Lock order is always playing → party → library/devices, from commands and engine events alike.
pub struct PlaybackState {
    pub(crate) player: Player,
    app: AppHandle,
    playing: Mutex<Option<Playing>>,
    modes: Mutex<Modes>,
    party: Mutex<PartyShuffle>,
    /// Bumped on every start, so a slow iCloud download can't play a song the user skipped.
    starts: AtomicU64,
    /// Bumped on every queue_following, so a slow prefetch can't queue a stale next song.
    nexts: AtomicU64,
    sound_check: AtomicBool,
}

impl PlaybackState {
    pub fn new(player: Player, app: AppHandle) -> Self {
        Self {
            player,
            app,
            starts: AtomicU64::new(0),
            nexts: AtomicU64::new(0),
            sound_check: AtomicBool::new(false),
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

    /// Sound Check's adjustment for a library song; songs on iPods and CDs play as they are.
    fn gain(&self, tracks: &Tracks, source: &PlaySource, track_id: i64) -> Option<Gain> {
        if !self.sound_check.load(Ordering::Relaxed) || *source != PlaySource::Library {
            return None;
        }
        let gain = tracks.library.queries().sound_check_gain(track_id, GainMode::Track, SOUND_CHECK_LUFS).ok()??;
        // An unknown peak counts as full scale: cuts apply, boosts don't.
        Some(Gain { db: gain.db as f32, peak_dbtp: gain.peak_dbtp.unwrap_or(0.0) as f32 })
    }

    /// Where a queued song's audio is, and whether it must be fetched before the engine can open
    /// it without blocking.
    fn locate(&self, tracks: &Tracks, source: &PlaySource, track_id: i64) -> CommandResult<Located> {
        if let PlaySource::Shared { library } = source {
            let (client, song_id, library, cache) = self.app.state::<RemoteState>().song(library, track_id)?;
            return Ok(Located::Shared { client, song_id, library, cache });
        }
        let path = tracks.path(source, track_id)?;
        Ok(if antistreaming_library::is_dataless(&path) { Located::ICloud(path) } else { Located::Ready(path) })
    }

    pub(crate) fn start(&self, tracks: &Tracks, playing: &Playing) -> CommandResult<()> {
        let track_id = playing.queue.current();
        let located = self.locate(tracks, &playing.source, track_id)?;
        let gain = self.gain(tracks, &playing.source, track_id);
        let start = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
        if let Located::Ready(path) = located {
            self.player.play_with(track_id, path, gain);
            return self.queue_following(tracks, playing);
        }
        // Opening a file that isn't here yet would block the engine while it arrives, so fetch it
        // first and start playing once it's local, showing progress in the LCD meanwhile.
        self.player.stop();
        let next = match playing.queue.peek_next() {
            Some(next) => Some((next, self.locate(tracks, &playing.source, next)?, self.gain(tracks, &playing.source, next))),
            None => None,
        };
        let app = self.app.clone();
        std::thread::spawn(move || {
            let from = located.from();
            let result = located.fetch(&app, track_id, &mut |done, total| {
                let _ = app.emit(DOWNLOADING_EVENT, Some(Downloading { track_id, done, total, from: from.clone() }));
            });
            let _ = app.emit(DOWNLOADING_EVENT, None::<Downloading>);
            let playback = app.state::<PlaybackState>();
            if playback.starts.load(Ordering::SeqCst) != start {
                return;
            }
            match result {
                Ok(path) => {
                    playback.player.play_with(track_id, path, gain);
                    if let Some((next, next_located, next_gain)) = next {
                        playback.queue_located(next, next_located, next_gain);
                    }
                }
                Err(error) => {
                    let _ = app.emit(PLAYER_EVENT, PlayerEvent::Notice { message: format!("Couldn't download from {from}: {error}") });
                }
            }
        });
        Ok(())
    }

    /// Hands the engine the next track early so it can play gaplessly.
    pub(crate) fn queue_following(&self, tracks: &Tracks, playing: &Playing) -> CommandResult<()> {
        match playing.queue.peek_next() {
            Some(next) => self.queue_located(next, self.locate(tracks, &playing.source, next)?, self.gain(tracks, &playing.source, next)),
            None => {
                self.nexts.fetch_add(1, Ordering::SeqCst);
                self.player.clear_next();
            }
        }
        Ok(())
    }

    /// Queues the next song for gapless play, fetching it first when needed so the engine never
    /// decodes ahead into a file that isn't here yet.
    fn queue_located(&self, track_id: i64, located: Located, gain: Option<Gain>) {
        let next = self.nexts.fetch_add(1, Ordering::SeqCst) + 1;
        if let Located::Ready(path) = located {
            self.player.queue_next_with(track_id, path, gain);
            return;
        }
        self.player.clear_next();
        let app = self.app.clone();
        std::thread::spawn(move || {
            let fetched = located.fetch(&app, track_id, &mut |_, _| {});
            let playback = app.state::<PlaybackState>();
            if let Ok(path) = fetched
                && playback.nexts.load(Ordering::SeqCst) == next
            {
                playback.player.queue_next_with(track_id, path, gain);
            }
        });
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

/// Where a queued song's audio is: ready to open, or still to be fetched before the engine can
/// open it without blocking.
enum Located {
    Ready(PathBuf),
    ICloud(PathBuf),
    Shared { client: Arc<Client>, song_id: String, library: String, cache: PathBuf },
}

impl Located {
    /// Where the LCD says a download comes from.
    fn from(&self) -> String {
        match self {
            Self::Ready(_) => String::new(),
            Self::ICloud(_) => "iCloud".to_owned(),
            Self::Shared { library, .. } => library.clone(),
        }
    }

    /// Blocks until the song is a local file, reporting (bytes so far, total) as it arrives.
    fn fetch(self, app: &AppHandle, track_id: i64, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
        match self {
            Self::Ready(path) => Ok(path),
            Self::ICloud(path) => {
                antistreaming_library::ensure_local(&path, progress, &AtomicBool::new(false)).map_err(|e| e.to_string())?;
                mark_local(app, track_id);
                Ok(path)
            }
            Self::Shared { client, song_id, cache, .. } => {
                let download = client.start_download(&song_id, &cache).map_err(|e| e.to_string())?;
                loop {
                    let state = download.progress();
                    progress(state.downloaded, state.total.unwrap_or(0));
                    if state.finished {
                        break;
                    }
                    download.wait_for(state.downloaded + PROGRESS_STEP_BYTES).map_err(|e| e.to_string())?;
                }
                download.wait().map_err(|e| e.to_string())
            }
        }
    }
}

/// How often a shared song's download reports progress.
const PROGRESS_STEP_BYTES: u64 = 256 * 1024;

/// A song that just came down from iCloud gets its real tags read and loses its cloud badge.
fn mark_local(app: &AppHandle, track_id: i64) {
    let refreshed = app.state::<LibraryState>().queries().refresh_cloud_status(&[track_id]);
    if refreshed.is_ok_and(|changed| !changed.is_empty()) {
        let _ = app.emit(crate::library::CHANGED_EVENT, ());
    }
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The engine stops decoding while its callback runs, so the callback only enqueues. This thread
/// does the work that can wait on SQLite or the webview, in the order events happened.
pub fn spawn_event_worker(app: AppHandle) -> std::io::Result<Sender<PlayerEvent>> {
    let (events, received) = mpsc::channel();
    std::thread::Builder::new().name("antistreaming-player-events".into()).spawn(move || {
        for event in received {
            handle_event(&app, event);
        }
    })?;
    Ok(events)
}

/// Forwards every event to the UI and keeps the queue in step.
fn handle_event(app: &AppHandle, event: PlayerEvent) {
    let _ = app.emit(PLAYER_EVENT, &event);
    let (Some(playback), Some(library), Some(devices)) =
        (app.try_state::<PlaybackState>(), app.try_state::<LibraryState>(), app.try_state::<DevicesState>())
    else {
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
        PlayerEvent::TrackEnded { track_id } => {
            if let PlaySource::Shared { library } = &playing.source {
                app.state::<RemoteState>().scrobble(library, track_id);
            }
            Ok(())
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

/// iTunes' Sound Check: levels songs to one loudness from the next song on, measuring in the
/// background whatever the tags don't already state.
#[tauri::command]
pub fn player_set_sound_check(app: AppHandle, playback: State<'_, PlaybackState>, enabled: bool) {
    playback.sound_check.store(enabled, Ordering::Relaxed);
    crate::loudness::set_measuring(&app, enabled);
}

#[tauri::command]
pub fn player_status(playback: State<'_, PlaybackState>) -> PlaybackStatus {
    playback.player.status()
}
