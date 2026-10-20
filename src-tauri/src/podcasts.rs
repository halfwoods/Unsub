use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};

use antistreaming_library::{MediaKind, TrackOverrides};
use antistreaming_podcasts::{Episode, Feed, FeedSettings, Podcasts};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{CommandError, CommandResult};
use crate::library::{self, LibraryState};

pub const CHANGED_EVENT: &str = "podcasts://changed";
pub const PROGRESS_EVENT: &str = "podcasts://progress";

fn to_error(error: antistreaming_podcasts::Error) -> CommandError {
    CommandError::from(error.to_string())
}

/// Podcast subscriptions, plus which library track each downloaded episode became.
pub struct PodcastsState {
    db_path: PathBuf,
    download_dir: PathBuf,
    store: Mutex<Podcasts>,
    tracks_path: PathBuf,
    /// episode id → library track id, so played state and resume position follow the song.
    tracks: Mutex<HashMap<i64, i64>>,
    /// Downloads run one at a time on their own store handle; the UI stays responsive meanwhile.
    download_lock: Arc<Mutex<()>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl PodcastsState {
    pub fn open(data_dir: &std::path::Path, download_dir: PathBuf) -> antistreaming_podcasts::Result<Self> {
        let db_path = data_dir.join("Podcasts.sqlite");
        let tracks_path = data_dir.join("PodcastTracks.json");
        let tracks = std::fs::read(&tracks_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Ok(Self {
            store: Mutex::new(Podcasts::open(&db_path, &download_dir)?),
            db_path,
            download_dir,
            tracks_path,
            tracks: Mutex::new(tracks),
            download_lock: Arc::new(Mutex::new(())),
        })
    }

    fn remember(&self, episode_id: i64, track_id: i64) {
        let mut tracks = lock(&self.tracks);
        tracks.insert(episode_id, track_id);
        if let Ok(json) = serde_json::to_vec(&*tracks) {
            let _ = std::fs::write(&self.tracks_path, json);
        }
    }

    pub fn episode_for_track(&self, track_id: i64) -> Option<i64> {
        lock(&self.tracks).iter().find(|(_, t)| **t == track_id).map(|(e, _)| *e)
    }

    /// Engine hook: a podcast that plays to the end is marked played, as in iTunes.
    pub fn track_finished(&self, track_id: i64) {
        if let Some(episode) = self.episode_for_track(track_id) {
            let _ = lock(&self.store).set_played(episode, true);
        }
    }

    /// Engine hook: remember where the listener is so the episode resumes there. Position events
    /// arrive four times a second; saving once per ten-second window is plenty.
    pub fn track_position(&self, track_id: i64, position_ms: u64) {
        if position_ms % 10_000 >= 300 {
            return;
        }
        if let Some(episode) = self.episode_for_track(track_id) {
            let _ = lock(&self.store).set_position(episode, position_ms);
        }
    }

    /// Where an unfinished episode left off, for resuming when it's played again.
    pub fn resume_position(&self, track_id: i64) -> Option<u64> {
        let episode_id = self.episode_for_track(track_id)?;
        let store = lock(&self.store);
        let episode =
            store.feeds().ok()?.into_iter().find_map(|feed| store.episodes(feed.id).ok()?.into_iter().find(|e| e.id == episode_id))?;
        (!episode.played && episode.position_ms > 0).then_some(episode.position_ms)
    }

    /// Keep-rules delete old episode files; their library rows go too so nothing points at a gap.
    fn forget_missing(&self, library: &LibraryState) {
        let gone: Vec<(i64, i64)> = lock(&self.tracks)
            .iter()
            .map(|(episode, track)| (*episode, *track))
            .filter(|&(_, track)| library.queries().track(track).ok().flatten().is_none_or(|t| !std::path::Path::new(&t.path).exists()))
            .collect();
        if gone.is_empty() {
            return;
        }
        let _ = library.queries().remove_tracks(&gone.iter().map(|&(_, track)| track).collect::<Vec<_>>());
        let mut tracks = lock(&self.tracks);
        for (episode, _) in &gone {
            tracks.remove(episode);
        }
        if let Ok(json) = serde_json::to_vec(&*tracks) {
            let _ = std::fs::write(&self.tracks_path, json);
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeView {
    #[serde(flatten)]
    episode: Episode,
    track_id: Option<i64>,
}

#[tauri::command]
pub fn podcasts_feeds(state: State<'_, PodcastsState>) -> CommandResult<Vec<Feed>> {
    lock(&state.store).feeds().map_err(to_error)
}

#[tauri::command]
pub fn podcasts_episodes(state: State<'_, PodcastsState>, feed_id: i64) -> CommandResult<Vec<EpisodeView>> {
    let episodes = lock(&state.store).episodes(feed_id).map_err(to_error)?;
    let tracks = lock(&state.tracks);
    Ok(episodes.into_iter().map(|episode| EpisodeView { track_id: tracks.get(&episode.id).copied(), episode }).collect())
}

#[tauri::command]
pub async fn podcasts_subscribe(app: AppHandle, state: State<'_, PodcastsState>, url: String) -> CommandResult<Feed> {
    let feed = lock(&state.store).subscribe(url.trim()).map_err(to_error)?;
    let _ = app.emit(CHANGED_EVENT, ());
    queue_auto_downloads(&app);
    Ok(feed)
}

#[tauri::command]
pub fn podcasts_unsubscribe(app: AppHandle, state: State<'_, PodcastsState>, feed_id: i64, delete_files: bool) -> CommandResult<()> {
    lock(&state.store).unsubscribe(feed_id, delete_files).map_err(to_error)?;
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(())
}

/// Refreshes one feed or all of them, then fetches whatever the auto-download rules now want.
#[tauri::command]
pub async fn podcasts_refresh(app: AppHandle, state: State<'_, PodcastsState>, feed_id: Option<i64>) -> CommandResult<()> {
    {
        let mut store = lock(&state.store);
        match feed_id {
            Some(id) => {
                store.refresh(id).map_err(to_error)?;
            }
            None => {
                store.refresh_all().map_err(to_error)?;
            }
        }
    }
    let _ = app.emit(CHANGED_EVENT, ());
    queue_auto_downloads(&app);
    Ok(())
}

#[tauri::command]
pub fn podcasts_set_settings(state: State<'_, PodcastsState>, feed_id: i64, settings: FeedSettings) -> CommandResult<()> {
    lock(&state.store).set_settings(feed_id, settings).map_err(to_error)
}

#[tauri::command]
pub fn podcasts_set_played(app: AppHandle, state: State<'_, PodcastsState>, episode_id: i64, played: bool) -> CommandResult<()> {
    lock(&state.store).set_played(episode_id, played).map_err(to_error)?;
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(())
}

#[tauri::command]
pub fn podcasts_download(app: AppHandle, episode_ids: Vec<i64>) {
    download(&app, episode_ids);
}

fn queue_auto_downloads(app: &AppHandle) {
    let pending = lock(&app.state::<PodcastsState>().store).pending_auto_downloads().unwrap_or_default();
    if !pending.is_empty() {
        download(app, pending);
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    episode_id: i64,
    done: u64,
    total: Option<u64>,
}

/// Downloads on a background thread with a second store handle (WAL makes that safe), then adds
/// each episode to the library as a Podcast titled from the feed rather than whatever tags it has.
fn download(app: &AppHandle, episode_ids: Vec<i64>) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<PodcastsState>();
        let _turn = lock(&state.download_lock);
        let mut store = match Podcasts::open(&state.db_path, &state.download_dir) {
            Ok(store) => store,
            Err(error) => return eprintln!("[podcasts] {error}"),
        };
        let feeds: HashMap<i64, Feed> = store.feeds().unwrap_or_default().into_iter().map(|f| (f.id, f)).collect();
        let cancel = AtomicBool::new(false);
        for episode_id in episode_ids {
            let result = store.download(
                episode_id,
                &mut |done, total| {
                    let _ = app.emit(PROGRESS_EVENT, Progress { episode_id, done, total });
                },
                &cancel,
            );
            let file = match result {
                Ok(file) => file,
                Err(error) => {
                    eprintln!("[podcasts] episode {episode_id}: {error}");
                    continue;
                }
            };
            let Some(episode) =
                feeds.values().find_map(|feed| store.episodes(feed.id).ok()?.into_iter().find(|e| e.id == episode_id).map(|e| (feed, e)))
            else {
                continue;
            };
            let (feed, episode) = episode;
            let overrides = TrackOverrides {
                kind: Some(MediaKind::Podcast),
                title: Some(episode.title.clone()),
                artist: Some(feed.author.clone()).filter(|a| !a.is_empty()),
                album: Some(feed.title.clone()),
                album_artist: Some(feed.author.clone()).filter(|a| !a.is_empty()),
                genre: Some("Podcast".to_owned()),
                date_added: episode.published,
                year: None,
                ..TrackOverrides::default()
            };
            let added = app.state::<LibraryState>().queries().add_files(&[(file, overrides)]);
            match added {
                Ok(summary) => {
                    if let Some(track_id) = summary.ids.first().copied().flatten() {
                        state.remember(episode_id, track_id);
                    }
                }
                Err(error) => eprintln!("[podcasts] could not add episode {episode_id} to the library: {error}"),
            }
            let _ = app.emit(CHANGED_EVENT, ());
            let _ = app.emit(library::CHANGED_EVENT, ());
        }
        if store.apply_cleanup().is_ok_and(|deleted| !deleted.is_empty()) {
            state.forget_missing(&app.state::<LibraryState>());
            let _ = app.emit(library::CHANGED_EVENT, ());
        }
    });
}
