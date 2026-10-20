use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use antistreaming_ipod::{Ipod, IpodMediaKind, SyncPlaylist, SyncProgress, SyncSupport, SyncTrack};
use antistreaming_library::{Library, MediaKind, PlaylistKind, SortKey, Track, TrackQuery};
use antistreaming_playback::{Target, TranscodeOptions};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::devices::{self, DevicesState};
use crate::error::{CommandError, CommandResult};
use crate::library::{self, LibraryState};
use crate::playback::{PlaySource, PlaybackState};

/// Progress while an iPod syncs; `null` once it's done.
pub const SYNC_EVENT: &str = "devices://sync";

/// iPods play Apple Lossless up to 48 kHz and 16 bits.
const IPOD_MAX_SAMPLE_RATE: u32 = 48_000;
const IPOD_MAX_BIT_DEPTH: u8 = 16;
/// What lossy files the iPod can't play become, when no conversion is chosen.
const FALLBACK_AAC_KBPS: u32 = 256;
const LOSSY_CODECS: [&str; 4] = ["AAC", "MP3", "Vorbis", "Opus"];
const IPOD_CODECS: [&str; 5] = ["AAC", "MP3", "ALAC", "WAV", "AIFF"];

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
        }
    }
}

/// Sync settings per iPod, kept in DeviceSync.json, and the sync in progress if any.
pub struct SyncState {
    path: PathBuf,
    settings: Mutex<HashMap<String, SyncSettings>>,
    running: Mutex<Option<Arc<AtomicBool>>>,
}

impl SyncState {
    pub fn load(path: PathBuf) -> Self {
        let settings = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { path, settings: Mutex::new(settings), running: Mutex::new(None) }
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Conversion {
    Copy,
    Alac,
    Aac(u32),
}

impl Conversion {
    /// Decided from the library's details so a preview needn't open thousands of files; the
    /// actual file is checked again just before copying.
    fn for_track(track: &Track, convert_kbps: Option<u32>) -> Self {
        let lossless = !LOSSY_CODECS.contains(&track.codec.as_str());
        if let Some(kbps) = convert_kbps
            && (lossless || track.bitrate_kbps.is_some_and(|rate| rate > kbps))
        {
            return Self::Aac(kbps);
        }
        let fits = track.sample_rate.is_none_or(|rate| rate <= IPOD_MAX_SAMPLE_RATE)
            && track.bit_depth.is_none_or(|bits| bits <= IPOD_MAX_BIT_DEPTH)
            && track.channels.is_none_or(|channels| channels <= 2);
        if IPOD_CODECS.contains(&track.codec.as_str()) && fits {
            Self::Copy
        } else if lossless {
            Self::Alac
        } else {
            Self::Aac(FALLBACK_AAC_KBPS)
        }
    }
}

/// The rate a converted file ends up at: the transcoder stays in the source's 44.1 or 48 kHz family.
fn ipod_rate(rate: u32) -> u32 {
    if rate <= IPOD_MAX_SAMPLE_RATE {
        rate
    } else if rate.is_multiple_of(44_100) {
        44_100
    } else {
        IPOD_MAX_SAMPLE_RATE
    }
}

/// What a sync would put on the iPod, and what it had to leave out.
struct Selection {
    tracks: Vec<SyncTrack>,
    conversions: HashMap<i64, (Conversion, bool)>,
    playlists: Vec<SyncPlaylist>,
    in_cloud: usize,
    missing: usize,
}

fn query(kind: Option<MediaKind>, playlist: Option<i64>) -> TrackQuery {
    TrackQuery {
        kind,
        playlist,
        sort: if playlist.is_some() { SortKey::PlaylistOrder } else { SortKey::Artist },
        ..TrackQuery::default()
    }
}

fn sync_track(track: &Track, conversion: Conversion) -> SyncTrack {
    let (bitrate_kbps, sample_rate) = match conversion {
        Conversion::Copy => (track.bitrate_kbps, track.sample_rate),
        Conversion::Alac => (None, track.sample_rate.map(ipod_rate)),
        Conversion::Aac(kbps) => (Some(kbps), track.sample_rate.map(ipod_rate)),
    };
    SyncTrack {
        source_path: PathBuf::from(&track.path),
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
        bitrate_kbps,
        sample_rate,
        rating: track.rating,
        play_count: track.play_count,
        last_played: track.last_played,
        date_added: track.date_added,
        media_kind: match track.kind {
            MediaKind::Music => IpodMediaKind::Music,
            MediaKind::Podcast => IpodMediaKind::Podcast,
            MediaKind::Audiobook => IpodMediaKind::Audiobook,
        },
        compilation: track.compilation,
        sort_title: track.sort_title.clone(),
        sort_artist: track.sort_artist.clone(),
        sort_album: track.sort_album.clone(),
        sort_album_artist: track.sort_album_artist.clone(),
        artwork: None,
        library_id: track.id,
    }
}

fn select(library: &Library, settings: &SyncSettings) -> antistreaming_library::Result<Selection> {
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

    let mut selection = Selection { tracks: Vec::new(), conversions: HashMap::new(), playlists: Vec::new(), in_cloud: 0, missing: 0 };
    let mut seen = HashSet::new();
    for track in candidates {
        if !seen.insert(track.id) || (settings.only_checked && !track.enabled) {
            continue;
        }
        if track.missing {
            selection.missing += 1;
            continue;
        }
        if track.in_cloud {
            selection.in_cloud += 1;
            continue;
        }
        let conversion = Conversion::for_track(&track, settings.convert_kbps);
        selection.conversions.insert(track.id, (conversion, !LOSSY_CODECS.contains(&track.codec.as_str())));
        selection.tracks.push(sync_track(&track, conversion));
    }
    if settings.music {
        for playlist in synced_playlists {
            let tracks: Vec<i64> = library
                .tracks(&query(None, Some(playlist.id)))?
                .into_iter()
                .map(|t| t.id)
                .filter(|id| selection.conversions.contains_key(id))
                .collect();
            selection.playlists.push(SyncPlaylist { name: playlist.name.clone(), tracks });
        }
    }
    Ok(selection)
}

/// The iPod as it is now: free space changes as music is added outside the app.
fn fresh_ipod(devices: &DevicesState, device_id: &str) -> CommandResult<Ipod> {
    let ipod = devices.ipod(device_id)?;
    let ipod = antistreaming_ipod::open(&ipod.mount_path).map_err(to_error)?;
    if !matches!(ipod.sync, SyncSupport::Supported | SyncSupport::NeedsHash58) {
        return Err(CommandError::from("AntiStreaming can't write to this iPod model's database yet".to_owned()));
    }
    Ok(ipod)
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
    in_cloud: usize,
    missing: usize,
}

/// What Sync would do with the current settings; touches nothing.
#[tauri::command]
pub async fn device_sync_preview(app: AppHandle, device_id: String) -> CommandResult<SyncPreview> {
    let ipod = fresh_ipod(&app.state::<DevicesState>(), &device_id)?;
    let settings = app.state::<SyncState>().settings_for(&device_id);
    let selection = select(&app.state::<LibraryState>().queries(), &settings)?;
    let database = antistreaming_ipod::read_database(&ipod).map_err(to_error)?;
    let plan = antistreaming_ipod::plan_sync(&ipod, &database, &selection.tracks, &selection.playlists).map_err(to_error)?;
    Ok(SyncPreview {
        tracks: selection.tracks.len(),
        add: plan.add.len(),
        update: plan.update.len(),
        remove: plan.remove.len(),
        bytes_needed: plan.bytes_needed,
        free_bytes: ipod.free_bytes,
        fits: plan.fits,
        converted: plan.add.iter().filter(|id| selection.conversions.get(id).is_some_and(|(c, _)| *c != Conversion::Copy)).count(),
        in_cloud: selection.in_cloud,
        missing: selection.missing,
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncStatus {
    device_id: String,
    #[serde(flatten)]
    progress: SyncProgress,
}

/// Merges what the iPod recorded (plays, skips, ratings) into the library, as iTunes did on
/// every sync, so the library's counts are what gets written back.
fn merge_play_counts(app: &AppHandle, ipod: &Ipod) -> CommandResult<bool> {
    let deltas = antistreaming_ipod::read_play_counts_for_sync_back(ipod).map_err(to_error)?;
    if deltas.is_empty() {
        return Ok(false);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let library = app.state::<LibraryState>();
    let mut library = library.queries();
    for (id, delta) in deltas {
        let at = delta.last_played.unwrap_or(now);
        for _ in 0..delta.plays {
            library.record_play(id, at)?;
        }
        for _ in 0..delta.skips {
            library.record_skip(id, at)?;
        }
        if let Some(rating) = delta.rating {
            library.set_rating(id, rating)?;
        }
    }
    Ok(true)
}

fn run_sync(app: &AppHandle, device_id: &str, cancel: &AtomicBool) -> CommandResult<()> {
    let devices = app.state::<DevicesState>();
    let ipod = fresh_ipod(&devices, device_id)?;
    // Songs playing from the iPod would hold files the sync may remove.
    app.state::<PlaybackState>().stop_if_playing(&PlaySource::Device { device_id: device_id.to_owned() });

    let merged = merge_play_counts(app, &ipod)?;
    let settings = app.state::<SyncState>().settings_for(device_id);
    let selection = select(&app.state::<LibraryState>().queries(), &settings)?;
    let database = antistreaming_ipod::read_database(&ipod).map_err(to_error)?;
    let plan = antistreaming_ipod::plan_sync(&ipod, &database, &selection.tracks, &selection.playlists).map_err(to_error)?;

    let work_dir = app.path().app_cache_dir().map_err(to_error)?.join("iPod Sync");
    // Clears what an interrupted sync may have left.
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir).map_err(to_error)?;
    // apply_sync copies each prepared file before asking for the next, so a converted file can
    // go as soon as the following one is requested: the cache never holds more than one song.
    let mut previous: Option<PathBuf> = None;
    let mut prepare = |track: &SyncTrack| -> io::Result<PathBuf> {
        if let Some(done) = previous.take() {
            let _ = std::fs::remove_file(done);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "the sync was cancelled"));
        }
        let (conversion, lossless) = selection.conversions.get(&track.library_id).copied().unwrap_or((Conversion::Copy, false));
        let target = match conversion {
            Conversion::Copy if antistreaming_playback::ipod_compatible(&track.source_path).unwrap_or(false) => {
                return Ok(track.source_path.clone());
            }
            // The library's details said it would play, but the file itself disagrees.
            Conversion::Copy if lossless => Target::Alac,
            Conversion::Copy => Target::Aac { kbps: FALLBACK_AAC_KBPS },
            Conversion::Alac => Target::Alac,
            Conversion::Aac(kbps) => Target::Aac { kbps },
        };
        let output = work_dir.join(format!("{}.m4a", track.library_id));
        let options = TranscodeOptions {
            target,
            max_sample_rate: Some(IPOD_MAX_SAMPLE_RATE),
            max_bit_depth: Some(IPOD_MAX_BIT_DEPTH),
            keep_tags: true,
        };
        antistreaming_playback::transcode_file(&track.source_path, &output, &options, &mut |_, _| {}, cancel)
            .map_err(|error| io::Error::other(error.to_string()))?;
        previous = Some(output.clone());
        Ok(output)
    };
    let mut progress = |progress: SyncProgress| {
        let _ = app.emit(SYNC_EVENT, Some(SyncStatus { device_id: device_id.to_owned(), progress }));
    };
    let result = antistreaming_ipod::apply_sync(&ipod, &plan, &selection.tracks, &selection.playlists, &mut prepare, &mut progress);
    if let Some(done) = previous {
        let _ = std::fs::remove_file(done);
    }

    // The library already holds what the iPod recorded, so the iPod's record goes whether or not
    // the sync committed; keeping it would count those plays again next time.
    if merged {
        antistreaming_ipod::clear_play_counts(&ipod).map_err(to_error)?;
        let _ = app.emit(library::CHANGED_EVENT, ());
    }
    result.map_err(to_error)?;
    devices.reconnect(&ipod.mount_path).map_err(to_error)?;
    let _ = app.emit(devices::CHANGED_EVENT, ());
    Ok(())
}

#[tauri::command]
pub fn device_sync(app: AppHandle, state: State<'_, SyncState>, device_id: String) -> CommandResult<()> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut running = lock(&state.running);
        if running.is_some() {
            return Err(CommandError::from("an iPod is already syncing".to_owned()));
        }
        *running = Some(Arc::clone(&cancel));
    }
    std::thread::spawn(move || {
        let result = run_sync(&app, &device_id, &cancel);
        *lock(&app.state::<SyncState>().running) = None;
        let _ = app.emit(SYNC_EVENT, None::<SyncStatus>);
        if let Err(error) = result {
            let message = if cancel.load(Ordering::Relaxed) { "Sync cancelled. Your iPod is as it was.".to_owned() } else { format!("Sync failed: {error}") };
            let _ = app.emit(crate::playback::PLAYER_EVENT, antistreaming_playback::PlayerEvent::Notice { message });
        }
    });
    Ok(())
}

#[tauri::command]
pub fn device_cancel_sync(state: State<'_, SyncState>) {
    if let Some(cancel) = lock(&state.running).as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, codec: &str, kind: MediaKind) -> Track {
        Track {
            id,
            path: format!("/music/{id}.{}", codec.to_lowercase()),
            title: format!("Song {id}"),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: "Album".into(),
            genre: String::new(),
            composer: String::new(),
            year: None,
            track_number: Some(1),
            track_count: None,
            disc_number: None,
            disc_count: None,
            duration_ms: 180_000,
            bitrate_kbps: Some(320),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            codec: codec.into(),
            file_size: 1_000_000,
            date_added: 0,
            date_modified: 0,
            play_count: 0,
            last_played: None,
            skip_count: 0,
            rating: 0,
            kind,
            enabled: true,
            in_cloud: false,
            missing: false,
            compilation: false,
            sort_title: None,
            sort_artist: None,
            sort_album: None,
            sort_album_artist: None,
        }
    }

    #[test]
    fn files_the_ipod_plays_are_copied_and_the_rest_converted() {
        let flac = track(1, "FLAC", MediaKind::Music);
        let hires_alac = Track { bit_depth: Some(24), sample_rate: Some(96_000), ..track(2, "ALAC", MediaKind::Music) };
        let surround = Track { channels: Some(6), ..track(3, "WAV", MediaKind::Music) };
        assert!(Conversion::for_track(&flac, None) == Conversion::Alac);
        assert!(Conversion::for_track(&hires_alac, None) == Conversion::Alac);
        assert!(Conversion::for_track(&surround, None) == Conversion::Alac);
        assert!(Conversion::for_track(&track(4, "ALAC", MediaKind::Music), None) == Conversion::Copy);
        assert!(Conversion::for_track(&track(5, "MP3", MediaKind::Music), None) == Conversion::Copy);
        assert!(Conversion::for_track(&track(6, "Opus", MediaKind::Music), None) == Conversion::Aac(FALLBACK_AAC_KBPS));
    }

    #[test]
    fn converting_to_aac_takes_lossless_and_higher_bit_rates_only() {
        let mp3_192 = Track { bitrate_kbps: Some(192), ..track(1, "MP3", MediaKind::Music) };
        assert!(Conversion::for_track(&track(2, "MP3", MediaKind::Music), Some(256)) == Conversion::Aac(256));
        assert!(Conversion::for_track(&mp3_192, Some(256)) == Conversion::Copy);
        assert!(Conversion::for_track(&track(3, "FLAC", MediaKind::Music), Some(128)) == Conversion::Aac(128));
    }

    #[test]
    fn converted_rates_stay_in_their_family() {
        assert_eq!(ipod_rate(44_100), 44_100);
        assert_eq!(ipod_rate(48_000), 48_000);
        assert_eq!(ipod_rate(88_200), 44_100);
        assert_eq!(ipod_rate(176_400), 44_100);
        assert_eq!(ipod_rate(96_000), 48_000);
        assert_eq!(ipod_rate(192_000), 48_000);
    }

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

    fn ids(selection: &Selection) -> Vec<i64> {
        let mut ids: Vec<i64> = selection.tracks.iter().map(|t| t.library_id).collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn the_whole_library_syncs_with_its_playlists_and_skips_what_it_cannot_copy() {
        let (library, _) = library();
        let selection = select(&library, &SyncSettings::default()).unwrap();
        assert_eq!(ids(&selection), [1, 2, 6, 7]);
        assert_eq!((selection.in_cloud, selection.missing), (1, 1));
        // Every playlist comes along, the default smart ones included, as in iTunes; each keeps
        // its order but lists only songs that are going onto the iPod.
        let names: Vec<&str> = selection.playlists.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Top 25 Most Played"));
        let road_trip = selection.playlists.iter().find(|p| p.name == "Road Trip").unwrap();
        assert_eq!(road_trip.tracks, [2, 1]);
    }

    #[test]
    fn only_checked_songs_and_chosen_playlists_narrow_the_sync() {
        let (library, playlist) = library();
        let checked = SyncSettings { only_checked: true, ..SyncSettings::default() };
        assert_eq!(ids(&select(&library, &checked).unwrap()), [1, 6, 7]);

        let chosen = SyncSettings {
            music_scope: MusicScope::Playlists { ids: vec![playlist] },
            podcasts: false,
            audiobooks: false,
            ..SyncSettings::default()
        };
        let selection = select(&library, &chosen).unwrap();
        assert_eq!(ids(&selection), [1, 2]);
        assert_eq!(selection.playlists[0].tracks, [2, 1]);
    }

    #[test]
    fn played_podcast_episodes_stay_behind_unless_asked_for() {
        let (library, _) = library();
        let music_off = SyncSettings { music: false, audiobooks: false, ..SyncSettings::default() };
        assert_eq!(ids(&select(&library, &music_off).unwrap()), [6]);
        let every_episode = SyncSettings { podcasts_unplayed_only: false, ..music_off };
        assert_eq!(ids(&select(&library, &every_episode).unwrap()), [5, 6]);
    }

    #[test]
    fn converted_songs_describe_the_file_the_ipod_gets() {
        let hires = Track { sample_rate: Some(96_000), bit_depth: Some(24), ..track(1, "FLAC", MediaKind::Music) };
        let alac = sync_track(&hires, Conversion::Alac);
        assert_eq!((alac.bitrate_kbps, alac.sample_rate), (None, Some(48_000)));
        let aac = sync_track(&hires, Conversion::Aac(256));
        assert_eq!((aac.bitrate_kbps, aac.sample_rate), (Some(256), Some(48_000)));
    }
}
