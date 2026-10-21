use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use unsub_devicesync::{Device as Player, Profile, SyncItem};
use unsub_ipod::{Ipod, IpodMediaKind, Prepared, SyncPlaylist, SyncProgress, SyncStage, SyncSupport, SyncTrack};
use unsub_library::{Library, MediaKind, PlaylistKind, SortKey, Track, TrackQuery};
use unsub_playback::{Target, TranscodeOptions};

use crate::devices::{self, DevicesState};
use crate::error::{CommandError, CommandResult};
use crate::library::{self, LibraryState};
use crate::playback::{PlaySource, PlaybackState};

/// Progress while an iPod or player syncs; `null` once it's done.
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

/// A sync or a copy off an iPod in progress: the device it is for, and the flag that stops it.
struct Running {
    device_id: String,
    cancel: Arc<AtomicBool>,
}

/// Sync settings per iPod, kept in DeviceSync.json, and the sync in progress if any.
pub struct SyncState {
    path: PathBuf,
    settings: Mutex<HashMap<String, SyncSettings>>,
    running: Mutex<Option<Running>>,
}

impl SyncState {
    pub fn load(path: PathBuf) -> Self {
        let settings = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { path, settings: Mutex::new(settings), running: Mutex::new(None) }
    }

    fn settings_for(&self, device_id: &str) -> SyncSettings {
        lock(&self.settings).get(device_id).cloned().unwrap_or_default()
    }

    /// Takes the one slot a sync or a copy off an iPod runs in, so the two can't run at once, and
    /// returns the flag that stops it. `finish` gives the slot back.
    pub(crate) fn begin(&self, device_id: &str) -> CommandResult<Arc<AtomicBool>> {
        let mut running = lock(&self.running);
        if running.is_some() {
            return Err(CommandError::from("Another sync or copy is still running.".to_owned()));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *running = Some(Running { device_id: device_id.to_owned(), cancel: Arc::clone(&cancel) });
        Ok(cancel)
    }

    pub(crate) fn finish(&self) {
        *lock(&self.running) = None;
    }

    /// Whether a sync or a copy is using this device, which then can't be taken away.
    pub(crate) fn is_busy_with(&self, device_id: &str) -> bool {
        lock(&self.running).as_ref().is_some_and(|running| running.device_id == device_id)
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

/// The library songs and playlists a sync takes, whatever the device, and what it left out.
struct Chosen {
    tracks: Vec<Track>,
    /// Each playlist's name and its songs in order, limited to the chosen songs.
    playlists: Vec<(String, Vec<i64>)>,
    /// Songs left out because they are only in iCloud, for a device that can't fetch them.
    in_cloud: usize,
    missing: usize,
}

/// The iPod's view of a selection: songs as its database describes them, and how each file is
/// made playable (the conversion, and whether the source is lossless).
struct IpodSelection {
    tracks: Vec<SyncTrack>,
    conversions: HashMap<i64, (Conversion, bool)>,
    playlists: Vec<SyncPlaylist>,
    /// The songs the library has down as only in iCloud, whose details are a placeholder's.
    in_cloud: HashSet<i64>,
}

fn is_lossless(track: &Track) -> bool {
    !LOSSY_CODECS.contains(&track.codec.as_str())
}

fn for_ipod(chosen: &Chosen, convert_kbps: Option<u32>) -> IpodSelection {
    let mut selection = IpodSelection { tracks: Vec::new(), conversions: HashMap::new(), playlists: Vec::new(), in_cloud: HashSet::new() };
    for track in &chosen.tracks {
        if track.in_cloud {
            selection.in_cloud.insert(track.id);
        }
        let conversion = Conversion::for_track(track, convert_kbps);
        selection.conversions.insert(track.id, (conversion, is_lossless(track)));
        selection.tracks.push(sync_track(track, conversion));
    }
    selection.playlists =
        chosen.playlists.iter().map(|(name, tracks)| SyncPlaylist { name: name.clone(), tracks: tracks.clone() }).collect();
    selection
}

/// What a player gets instead of a file it can't play: the profile's first format, made from
/// lossless sources losslessly where that format allows. `None` copies the file as it is.
fn player_target(track: &Track, profile: &Profile) -> Option<(Target, &'static str)> {
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

fn query(kind: Option<MediaKind>, playlist: Option<i64>) -> TrackQuery {
    TrackQuery { kind, playlist, sort: if playlist.is_some() { SortKey::PlaylistOrder } else { SortKey::Artist }, ..TrackQuery::default() }
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
            // `select` leaves stations out, so none reaches here.
            MediaKind::Radio => IpodMediaKind::Other,
        },
        compilation: track.compilation,
        sort_title: track.sort_title.clone(),
        sort_artist: track.sort_artist.clone(),
        sort_album: track.sort_album.clone(),
        sort_album_artist: track.sort_album_artist.clone(),
        artwork_key: Some(artwork_key(track)),
        // iTunes always wrote Sound Check values; the iPod's own setting decides whether to use them.
        sound_check: track.loudness.as_ref().map(|l| unsub_library::itunes_soundcheck(crate::playback::SOUND_CHECK_LUFS - l.lufs)),
        library_id: track.id,
    }
}

/// Covers live in the files (Get Info rewrites the file to change one), so the file's identity
/// is the cover's: unchanged files are never re-read for artwork on later syncs. A Rust update
/// that changes the hasher only costs one round of re-sent covers.
fn artwork_key(track: &Track) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (&track.path, track.file_size, track.date_modified).hash(&mut hasher);
    hasher.finish()
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

/// The iPod as it is now: free space changes as music is added outside the app.
fn fresh_ipod(devices: &DevicesState, device_id: &str) -> CommandResult<Ipod> {
    let ipod = devices.ipod(device_id)?;
    let ipod = unsub_ipod::open(&ipod.mount_path).map_err(to_error)?;
    if !matches!(ipod.sync, SyncSupport::Supported | SyncSupport::NeedsHash58) {
        return Err(CommandError::from("Unsub can't write to this iPod model's database yet".to_owned()));
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

fn for_player(chosen: &Chosen) -> (Vec<SyncItem>, Vec<unsub_devicesync::SyncPlaylist>) {
    let items = chosen.tracks.iter().map(sync_item).collect();
    let playlists =
        chosen.playlists.iter().map(|(name, items)| unsub_devicesync::SyncPlaylist { name: name.clone(), items: items.clone() }).collect();
    (items, playlists)
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncStatus {
    device_id: String,
    #[serde(flatten)]
    progress: SyncProgress,
}

/// How a sync reaches songs that are only in iCloud; a test stands in for the real thing.
trait Cloud {
    /// Whether the file is a placeholder whose contents aren't on this Mac.
    fn is_placeholder(&self, path: &Path) -> bool;
    /// Downloads it, reporting (bytes so far, total) and giving up once `stop` is set.
    fn download(&self, path: &Path, progress: &mut dyn FnMut(u64, u64), stop: &AtomicBool) -> unsub_library::Result<()>;
    /// Gives the downloaded copy back to iCloud, freeing the space.
    fn release(&self, path: &Path) -> unsub_library::Result<()>;
}

struct ICloud;

impl Cloud for ICloud {
    fn is_placeholder(&self, path: &Path) -> bool {
        unsub_library::is_dataless(path)
    }

    fn download(&self, path: &Path, progress: &mut dyn FnMut(u64, u64), stop: &AtomicBool) -> unsub_library::Result<()> {
        unsub_library::ensure_local(path, progress, stop)
    }

    fn release(&self, path: &Path) -> unsub_library::Result<()> {
        unsub_library::evict(path)
    }
}

/// How long a download may go without a new byte before the song is left out, at least. A Mac that
/// is offline never finishes a download or fails it, so without a limit the sync would wait for
/// ever; and since how much of a file is on disk may not move until it is all there, a big file
/// (an audiobook) is given longer, as if it came at [`SLOWEST_DOWNLOAD`].
const DOWNLOAD_STALL: Duration = Duration::from_secs(180);
/// Bytes a second, below which a download is not expected to be going.
const SLOWEST_DOWNLOAD: u64 = 256 * 1024;
/// Stalled downloads in a row after which none is tried.
const TIMEOUTS_BEFORE_OFFLINE: u8 = 3;
const TIMED_OUT: &str = "timed out waiting for iCloud";
const NO_RESPONSE: &str = "skipped because iCloud isn't responding";
const UNREADABLE: &str = "couldn't be read once downloaded";
const NOT_CONVERTED: &str = "couldn't be converted for the iPod";
/// Covers kept for the iPod are no wider than this; its screens show them far smaller.
const COVER_PIXELS: u32 = 640;
/// How many songs' cloud status the library refreshes at once, so it is never held for long.
const REFRESH_BATCH: usize = 500;
const LEFTOVERS_FILE: &str = "Device Sync Downloads.json";

fn stall_limit(floor: Duration, total_bytes: u64) -> Duration {
    floor.max(Duration::from_secs(total_bytes / SLOWEST_DOWNLOAD))
}

/// A download a sync asked iCloud for and then stopped waiting for, because it was stopped or
/// gave up. The request is still out there, so the song lands on this Mac whenever iCloud gets to
/// it, and nothing else would give it back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Leftover {
    path: PathBuf,
    /// The iPod whose sync asked, whose "remove downloaded songs" setting decides.
    device: String,
    /// Unix seconds.
    at: i64,
}

/// How long a song is waited for: a request iCloud has not answered in a day never will be, and
/// the song may have been downloaded and wanted since.
const LEFTOVER_LIFE_SECS: i64 = 24 * 60 * 60;

/// The [`Leftover`]s, kept in a small file in the app cache until a later sync or the next launch
/// finds them finished and gives them back.
struct Leftovers {
    file: PathBuf,
}

/// Held while the file is read and written back, since a sync, playback and launch all use it.
static LEFTOVERS: Mutex<()> = Mutex::new(());

impl Leftovers {
    fn load(&self, now: i64) -> Vec<Leftover> {
        let all: Vec<Leftover> = std::fs::read(&self.file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        all.into_iter().filter(|left| now - left.at < LEFTOVER_LIFE_SECS).collect()
    }

    /// A list that can't be saved is a download that stays on the Mac, not a failed sync.
    fn save(&self, all: &[Leftover]) {
        if all.is_empty() {
            let _ = std::fs::remove_file(&self.file);
        } else if let Ok(json) = serde_json::to_vec(all) {
            let _ = self.file.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(&self.file, json);
        }
    }

    fn add(&self, paths: &[PathBuf], device: &str, now: i64) {
        if paths.is_empty() {
            return;
        }
        let _held = lock(&LEFTOVERS);
        let mut all = self.load(now);
        all.retain(|left| !paths.contains(&left.path));
        all.extend(paths.iter().map(|path| Leftover { path: path.clone(), device: device.to_owned(), at: now }));
        self.save(&all);
    }

    /// The song is wanted on this Mac after all, as when it is played.
    fn forget(&self, path: &Path, now: i64) {
        let _held = lock(&LEFTOVERS);
        let mut all = self.load(now);
        let before = all.len();
        all.retain(|left| left.path != path);
        if all.len() != before {
            self.save(&all);
        }
    }

    /// Gives back the downloads that have finished since, where their iPod's setting says to,
    /// forgets those whose file is gone, and keeps waiting for the rest.
    fn release(&self, cloud: &dyn Cloud, giving_back: &dyn Fn(&str) -> bool, now: i64) {
        let _held = lock(&LEFTOVERS);
        let waiting: Vec<Leftover> = self
            .load(now)
            .into_iter()
            .filter(|left| {
                if !left.path.exists() {
                    return false;
                }
                if cloud.is_placeholder(&left.path) {
                    return true;
                }
                if giving_back(&left.device) {
                    let _ = cloud.release(&left.path);
                }
                false
            })
            .collect();
        self.save(&waiting);
    }
}

fn leftovers_of(app: &AppHandle) -> CommandResult<Leftovers> {
    Ok(Leftovers { file: app.path().app_cache_dir().map_err(to_error)?.join(LEFTOVERS_FILE) })
}

/// Whether the iPod's sync settings say to give downloaded songs back.
fn giving_back(app: &AppHandle) -> impl Fn(&str) -> bool + use<> {
    let app = app.clone();
    move |device| app.state::<SyncState>().settings_for(device).remove_downloaded
}

/// A song that was a leftover is wanted on this Mac now, so it is not to be given back.
pub fn forget_leftover(app: &AppHandle, path: &Path) {
    if let Ok(leftovers) = leftovers_of(app) {
        leftovers.forget(path, crate::playback::now_unix());
    }
}

/// Cleans up after a sync the app did not live to finish: clears the cache of converted songs and
/// covers, and gives back the downloads that finished after it stopped waiting for them.
pub fn tidy(app: &AppHandle) {
    let Ok(cache) = app.path().app_cache_dir() else { return };
    let _ = std::fs::remove_dir_all(cache.join(WORK_DIR));
    let giving_back = giving_back(app);
    // Each song is a call to the system, so not while the app is starting.
    std::thread::spawn(move || {
        Leftovers { file: cache.join(LEFTOVERS_FILE) }.release(&ICloud, &giving_back, crate::playback::now_unix());
    });
}

/// The covers of songs that came down from iCloud, read while the song is here: it is given back
/// before the iPod's artwork is written, and a cover can't be read from a placeholder. Songs of an
/// album mostly share one picture, so each distinct picture is stored once.
struct Covers {
    dir: PathBuf,
    by_song: RefCell<HashMap<i64, u64>>,
}

impl Covers {
    fn new(dir: PathBuf) -> Self {
        Self { dir, by_song: RefCell::default() }
    }

    fn file(&self, key: u64) -> PathBuf {
        self.dir.join(format!("{key:016x}"))
    }

    /// A cover that can't be kept costs the song its picture, not the sync.
    fn keep(&self, song: i64, picture: &[u8]) {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        picture.hash(&mut hasher);
        let key = hasher.finish();
        let file = self.file(key);
        if file.exists() || std::fs::create_dir_all(&self.dir).and_then(|()| std::fs::write(&file, picture)).is_ok() {
            self.by_song.borrow_mut().insert(song, key);
        }
    }

    fn get(&self, song: i64) -> Option<Vec<u8>> {
        let key = *self.by_song.borrow().get(&song)?;
        std::fs::read(self.file(key)).ok()
    }

    fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// What an iPod sync's songs go through just before each is copied: a song that is only in iCloud
/// is downloaded and read, then (if asked) given back to iCloud once it has been copied, so the
/// Mac holds about one song of the library at a time. A song that can't be had is left out.
struct IpodSongs<'a> {
    library: &'a LibraryState,
    cloud: &'a dyn Cloud,
    covers: &'a Covers,
    emit: &'a dyn Fn(SyncProgress),
    /// (done, total) as `apply_sync` last reported them for the song being prepared.
    position: &'a Cell<(u64, u64)>,
    conversions: &'a HashMap<i64, (Conversion, bool)>,
    /// Songs the library has down as only in iCloud. One that is here after all, downloaded some
    /// other way since, is only read.
    in_cloud: &'a HashSet<i64>,
    convert_kbps: Option<u32>,
    work_dir: &'a Path,
    cancel: &'a AtomicBool,
    /// Give each downloaded song back to iCloud after it is copied.
    release: bool,
    /// Whether the iPod gets album art, so whether a downloaded song's cover is worth keeping.
    artwork: bool,
    stall: Duration,
    /// Downloads in a row that stalled. Enough of them and iCloud is taken to be out of reach,
    /// and the rest are not tried.
    timeouts: u8,
    /// The converted file of the song just prepared. `apply_sync` copies each prepared file before
    /// asking for the next, so it goes as soon as the next is requested.
    converted: Option<PathBuf>,
    /// The song just prepared, if this sync downloaded it and is to give it back.
    downloaded: Option<PathBuf>,
    /// Downloads this sync asked for that did not finish, because it was stopped or gave up on
    /// them. iCloud may yet complete them, and then they stay on the Mac unless given back.
    abandoned: Vec<PathBuf>,
    /// Library ids of the songs that came down, whose cloud status needs refreshing at the end.
    fetched: Vec<i64>,
    /// Why each song that was left out was.
    skipped: Vec<String>,
}

/// What `IpodSongs` leaves behind when the sync is over.
struct Fetched {
    songs: usize,
    skipped: Vec<String>,
    /// Downloads still going that the sync asked for, for [`Leftovers`].
    unfinished: Vec<PathBuf>,
}

/// How a song's download ended.
enum Fetch {
    Here,
    /// It can't be had, and the sync goes on without it.
    Left,
    /// The user stopped the sync.
    Stop,
}

impl IpodSongs<'_> {
    fn prepare(&mut self, track: &SyncTrack) -> Prepared {
        self.release_downloaded();
        if let Some(done) = self.converted.take() {
            let _ = std::fs::remove_file(done);
        }
        if self.cancel.load(Ordering::Relaxed) {
            return Prepared::Stop;
        }

        let placeholder = self.cloud.is_placeholder(&track.source_path);
        let fetched = if placeholder || self.in_cloud.contains(&track.library_id) {
            if placeholder {
                match self.download(track) {
                    Fetch::Here => {}
                    Fetch::Left => return Prepared::Skip,
                    Fetch::Stop => return Prepared::Stop,
                }
            }
            match self.read(track) {
                Some(song) => Some(song),
                None => return Prepared::Skip,
            }
        } else {
            None
        };
        // A song that came down is described by its file, not by the placeholder the library had.
        let (conversion, lossless, described) = match fetched {
            Some(song) => {
                let conversion = Conversion::for_track(&song, self.convert_kbps);
                (conversion, is_lossless(&song), Some(sync_track(&song, conversion)))
            }
            None => {
                let (conversion, lossless) = self.conversions.get(&track.library_id).copied().unwrap_or((Conversion::Copy, false));
                (conversion, lossless, None)
            }
        };
        match playable_file(described.as_ref().unwrap_or(track), conversion, lossless, self.work_dir, self.cancel) {
            Ok(path) => {
                if path != track.source_path {
                    self.converted = Some(path.clone());
                }
                Prepared::File { path, track: described.map(Box::new) }
            }
            // A stopped conversion fails like any other; one song that won't convert must not end
            // a sync that may be hours in.
            Err(_) if self.cancel.load(Ordering::Relaxed) => Prepared::Stop,
            Err(_) => {
                self.skipped.push(NOT_CONVERTED.to_owned());
                Prepared::Skip
            }
        }
    }

    fn download(&mut self, track: &SyncTrack) -> Fetch {
        if self.timeouts >= TIMEOUTS_BEFORE_OFFLINE {
            self.skipped.push(NO_RESPONSE.to_owned());
            return Fetch::Left;
        }
        let (done, total) = self.position.get();
        (self.emit)(SyncProgress { stage: SyncStage::Downloading, done, total, current: track.title.clone() });
        let (cancel, floor) = (self.cancel, self.stall);
        let stop = AtomicBool::new(false);
        let mut last = (Instant::now(), 0);
        let mut watch = |bytes: u64, total: u64| {
            if bytes != last.1 {
                last = (Instant::now(), bytes);
            }
            if cancel.load(Ordering::Relaxed) || last.0.elapsed() > stall_limit(floor, total) {
                stop.store(true, Ordering::Relaxed);
            }
        };
        match self.cloud.download(&track.source_path, &mut watch, &stop) {
            Ok(()) => {
                self.timeouts = 0;
                if self.release {
                    self.downloaded = Some(track.source_path.clone());
                }
                Fetch::Here
            }
            Err(error) => {
                // Giving up on a download does not call off the request, which iCloud may still
                // answer later. A download that failed outright has no request left.
                let waiting = matches!(error, unsub_library::Error::Cancelled);
                if waiting && self.release {
                    self.abandoned.push(track.source_path.clone());
                }
                match error {
                    unsub_library::Error::Cancelled if cancel.load(Ordering::Relaxed) => Fetch::Stop,
                    // Not the user's Stop: the watch above gave up on a download that stopped moving.
                    unsub_library::Error::Cancelled => {
                        self.timeouts += 1;
                        self.skipped.push(TIMED_OUT.to_owned());
                        Fetch::Left
                    }
                    error => {
                        self.skipped.push(format!("couldn't be downloaded ({error})"));
                        Fetch::Left
                    }
                }
            }
        }
    }

    /// The song as its file describes it, which the library only learns once the file is here.
    /// `None` when the file can't be read and the song is left out.
    fn read(&mut self, track: &SyncTrack) -> Option<Track> {
        let id = track.library_id;
        let song = {
            let mut library = self.library.queries();
            let _ = library.refresh_cloud_status(&[id]);
            library.track(id).ok().flatten()
        };
        let Some(song) = song.filter(|song| !song.in_cloud) else {
            self.skipped.push(UNREADABLE.to_owned());
            return None;
        };
        // The original, shrunk here and kept only for this sync: the library's own thumbnails are
        // cached for good, and a song this sync gives back would leave its thumbnail for nothing.
        if self.artwork
            && let Some(cover) = self.library.artwork(id, None).and_then(|art| unsub_library::shrink_to_jpeg(&art.bytes, COVER_PIXELS))
        {
            self.covers.keep(id, &cover);
        }
        self.fetched.push(id);
        Some(song)
    }

    fn release_downloaded(&mut self) {
        if let Some(path) = self.downloaded.take() {
            // iCloud declines a song it hasn't taken the changes to yet; it stays, and nothing is lost.
            let _ = self.cloud.release(&path);
        }
    }

    /// Ends the sync's dealings with iCloud and the cache, finished or not: the last song goes
    /// back, so do abandoned downloads that have since completed, and the library learns which
    /// songs came down and which are cloud-only again.
    fn finish(&mut self) -> Fetched {
        self.release_downloaded();
        if let Some(done) = self.converted.take() {
            let _ = std::fs::remove_file(done);
        }
        let abandoned = std::mem::take(&mut self.abandoned);
        let (finished, unfinished): (Vec<PathBuf>, Vec<PathBuf>) = abandoned.into_iter().partition(|path| !self.cloud.is_placeholder(path));
        for path in &finished {
            let _ = self.cloud.release(path);
        }
        // A batch at a time, so the song list is never held up for the whole of a big refresh.
        for ids in self.fetched.chunks(REFRESH_BATCH) {
            let _ = self.library.queries().refresh_cloud_status(ids);
        }
        Fetched { songs: self.fetched.len(), skipped: std::mem::take(&mut self.skipped), unfinished }
    }
}

/// The file to copy for the song: itself when the iPod plays it as it is, else a conversion made
/// in `work_dir`.
fn playable_file(track: &SyncTrack, conversion: Conversion, lossless: bool, work_dir: &Path, cancel: &AtomicBool) -> io::Result<PathBuf> {
    let target = match conversion {
        Conversion::Copy if unsub_playback::ipod_compatible(&track.source_path).unwrap_or(false) => {
            return Ok(track.source_path.clone());
        }
        // The library's details said it would play, but the file itself disagrees.
        Conversion::Copy if lossless => Target::Alac,
        Conversion::Copy => Target::Aac { kbps: FALLBACK_AAC_KBPS },
        Conversion::Alac => Target::Alac,
        Conversion::Aac(kbps) => Target::Aac { kbps },
    };
    let output = work_dir.join(format!("{}.m4a", track.library_id));
    let options =
        TranscodeOptions { target, max_sample_rate: Some(IPOD_MAX_SAMPLE_RATE), max_bit_depth: Some(IPOD_MAX_BIT_DEPTH), keep_tags: true };
    unsub_playback::transcode_file(&track.source_path, &output, &options, &mut |_, _| {}, cancel)
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(output)
}

/// What to tell the user about songs a sync left out: how many, and why.
fn unsynced_notice(reasons: &[String]) -> String {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for reason in reasons {
        *counts.entry(reason).or_default() += 1;
    }
    let why: Vec<String> = counts.iter().map(|(reason, count)| format!("{count} {reason}")).collect();
    let (count, was) = (reasons.len(), if reasons.len() == 1 { "song wasn't" } else { "songs weren't" });
    format!("Synced, but {count} {was} copied: {}", why.join("; "))
}

/// Merges what the iPod recorded (plays, skips, ratings) into the library, as iTunes did on
/// every sync, so the library's counts are what gets written back.
fn merge_play_counts(app: &AppHandle, ipod: &Ipod) -> CommandResult<bool> {
    let deltas = unsub_ipod::read_play_counts_for_sync_back(ipod).map_err(to_error)?;
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

/// Copies the chosen songs onto a non-Apple player as files and playlists. Its manifest records
/// each copy as it lands, so a stopped sync picks up where it left off next time.
fn run_player_sync(app: &AppHandle, player: &Player, cancel: &AtomicBool) -> CommandResult<()> {
    let settings = app.state::<SyncState>().settings_for(&player.id);
    let chosen = select(&app.state::<LibraryState>().queries(), &settings, false)?;
    let work_dir = fresh_work_dir(app)?;
    let summary = sync_to_player(player, &chosen, &work_dir, cancel, &mut |progress| {
        let _ = app.emit(SYNC_EVENT, Some(PlayerSyncStatus { device_id: player.id.clone(), progress }));
    })?;
    app.state::<DevicesState>().rediscover_players();
    let _ = app.emit(devices::CHANGED_EVENT, ());
    // Songs the player couldn't take are the one thing worth reporting after a good sync.
    if !summary.skipped.is_empty() {
        let count = summary.skipped.len();
        let message = format!("Synced, but {count} {} couldn't be converted for the player", if count == 1 { "song" } else { "songs" });
        let _ = app.emit(crate::playback::PLAYER_EVENT, unsub_playback::PlayerEvent::Notice { message });
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

fn run_sync(app: &AppHandle, device_id: &str, cancel: &AtomicBool) -> CommandResult<()> {
    if let Some(player) = app.state::<DevicesState>().player(device_id) {
        return run_player_sync(app, &player, cancel);
    }
    let devices = app.state::<DevicesState>();
    let ipod = fresh_ipod(&devices, device_id)?;
    // Songs playing from the iPod would hold files the sync may remove.
    app.state::<PlaybackState>().stop_if_playing(&PlaySource::Device { device_id: device_id.to_owned() });

    // The library holds what the iPod recorded once this has merged it, so the iPod's record goes
    // at once: left until the end it would be counted again by the next sync after any failure.
    if merge_play_counts(app, &ipod)? {
        unsub_ipod::clear_play_counts(&ipod).map_err(to_error)?;
        let _ = app.emit(library::CHANGED_EVENT, ());
    }
    let settings = app.state::<SyncState>().settings_for(device_id);
    let selection = for_ipod(&select(&app.state::<LibraryState>().queries(), &settings, true)?, settings.convert_kbps);
    let database = unsub_ipod::read_database(&ipod).map_err(to_error)?;
    let plan = unsub_ipod::plan_sync(&ipod, &database, &selection.tracks, &selection.playlists).map_err(to_error)?;

    let leftovers = leftovers_of(app)?;
    leftovers.release(&ICloud, &giving_back(app), crate::playback::now_unix());
    let work_dir = fresh_work_dir(app)?;
    let library = app.state::<LibraryState>();
    let covers = Covers::new(work_dir.join("covers"));
    let position = Cell::new((0, 0));
    let emit = |progress: SyncProgress| {
        let _ = app.emit(SYNC_EVENT, Some(SyncStatus { device_id: device_id.to_owned(), progress }));
    };
    let mut songs = IpodSongs {
        library: &library,
        cloud: &ICloud,
        covers: &covers,
        emit: &emit,
        position: &position,
        conversions: &selection.conversions,
        in_cloud: &selection.in_cloud,
        convert_kbps: settings.convert_kbps,
        work_dir: &work_dir,
        cancel,
        release: settings.remove_downloaded,
        artwork: unsub_ipod::artwork_supported(&ipod),
        stall: DOWNLOAD_STALL,
        timeouts: 0,
        converted: None,
        downloaded: None,
        abandoned: Vec::new(),
        fetched: Vec::new(),
        skipped: Vec::new(),
    };
    let mut prepare = |track: &SyncTrack| Ok(songs.prepare(track));
    let mut progress = |progress: SyncProgress| {
        if progress.stage == SyncStage::Preparing {
            position.set((progress.done, progress.total));
        }
        emit(progress);
    };
    // A downloaded song's cover was read while it was here; the rest are read from the library now.
    let mut cover = |library_id: i64| covers.get(library_id).or_else(|| library.artwork(library_id, None).map(|art| art.bytes));
    let result = unsub_ipod::apply_sync(&ipod, &plan, &selection.tracks, &selection.playlists, &mut prepare, &mut cover, &mut progress);
    let fetched = songs.finish();
    leftovers.add(&fetched.unfinished, device_id, crate::playback::now_unix());
    covers.clear();
    if fetched.songs > 0 {
        let _ = app.emit(library::CHANGED_EVENT, ());
    }

    // Whatever became of the sync, songs may have been committed, and the view of the iPod is of
    // what it held before.
    let reconnected = devices.reconnect(&ipod.mount_path);
    let _ = app.emit(devices::CHANGED_EVENT, ());
    result.map_err(to_error)?;
    reconnected.map_err(to_error)?;
    // A stop commits what was copied, so it ends like a sync and the notice is the one place to say so.
    let message = if cancel.load(Ordering::Relaxed) {
        // Songs are removed only at the end of a sync, unless there was no room without.
        let removed = if plan.removals_first && !plan.remove.is_empty() {
            let count = plan.remove.len();
            format!(" {count} {} removed to make room.", if count == 1 { "song was" } else { "songs were" })
        } else {
            String::new()
        };
        Some(format!("Sync stopped. The songs already copied are on the iPod.{removed}"))
    } else if fetched.skipped.is_empty() {
        None
    } else {
        Some(unsynced_notice(&fetched.skipped))
    };
    if let Some(message) = message {
        let _ = app.emit(crate::playback::PLAYER_EVENT, unsub_playback::PlayerEvent::Notice { message });
    }
    Ok(())
}

#[tauri::command]
pub fn device_sync(app: AppHandle, state: State<'_, SyncState>, device_id: String) -> CommandResult<()> {
    let cancel = state.begin(&device_id)?;
    std::thread::spawn(move || {
        let result = run_sync(&app, &device_id, &cancel);
        app.state::<SyncState>().finish();
        let _ = app.emit(SYNC_EVENT, None::<SyncStatus>);
        if let Err(error) = result {
            let message = if cancel.load(Ordering::Relaxed) { "Sync stopped".to_owned() } else { format!("Sync failed: {error}") };
            let _ = app.emit(crate::playback::PLAYER_EVENT, unsub_playback::PlayerEvent::Notice { message });
        }
    });
    Ok(())
}

#[tauri::command]
pub fn device_cancel_sync(state: State<'_, SyncState>) {
    if let Some(running) = lock(&state.running).as_ref() {
        running.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::track;

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

    #[test]
    fn converted_songs_describe_the_file_the_ipod_gets() {
        let hires = Track { sample_rate: Some(96_000), bit_depth: Some(24), ..track(1, "FLAC", MediaKind::Music) };
        let alac = sync_track(&hires, Conversion::Alac);
        assert_eq!((alac.bitrate_kbps, alac.sample_rate), (None, Some(48_000)));
        let aac = sync_track(&hires, Conversion::Aac(256));
        assert_eq!((aac.bitrate_kbps, aac.sample_rate), (Some(256), Some(48_000)));
    }
    /// A cloud whose songs are the files in `placeholders` until they are downloaded.
    #[derive(Default)]
    struct FakeCloud<'a> {
        placeholders: RefCell<HashSet<PathBuf>>,
        downloads: RefCell<Vec<PathBuf>>,
        released: RefCell<Vec<PathBuf>>,
        /// Downloads of these fail, as when the file is gone or the account signed out.
        failing: HashSet<PathBuf>,
        /// Downloads of these never make progress, as when the Mac is offline.
        hanging: HashSet<PathBuf>,
        /// A download that makes no progress for this long before it completes, as when the Mac is
        /// offline (for ever, with `Duration::MAX`) or the file is big and slow.
        slow: Option<Duration>,
        /// The size the download reports, 100 bytes if not given.
        total: Option<u64>,
        /// Sets this flag as soon as a download starts, as a click on Stop would.
        cancels: Option<&'a AtomicBool>,
    }

    impl Cloud for FakeCloud<'_> {
        fn is_placeholder(&self, path: &Path) -> bool {
            self.placeholders.borrow().contains(path)
        }

        fn download(&self, path: &Path, progress: &mut dyn FnMut(u64, u64), stop: &AtomicBool) -> unsub_library::Result<()> {
            self.downloads.borrow_mut().push(path.to_owned());
            if let Some(cancel) = self.cancels {
                cancel.store(true, Ordering::Relaxed);
            }
            if self.failing.contains(path) {
                return Err(io::Error::other("offline").into());
            }
            let total = self.total.unwrap_or(100);
            let started = Instant::now();
            let slow = if self.hanging.contains(path) { Some(Duration::MAX) } else { self.slow };
            while slow.is_some_and(|slow| started.elapsed() < slow) {
                progress(0, total);
                if stop.load(Ordering::Relaxed) {
                    return Err(unsub_library::Error::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            self.placeholders.borrow_mut().remove(path);
            progress(total, total);
            Ok(())
        }

        fn release(&self, path: &Path) -> unsub_library::Result<()> {
            self.released.borrow_mut().push(path.to_owned());
            Ok(())
        }
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/playback/tests/fixtures").join(name)
    }

    /// 1x1 PNGs, which the cover code can decode and shrink into a JPEG.
    const RED: [u8; 69] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63,
        0x38, 0xc1, 0xc0, 0x00, 0x00, 0x02, 0x5c, 0x00, 0xc9, 0x43, 0xc8, 0x12, 0x46, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
        0x42, 0x60, 0x82,
    ];
    const BLUE: [u8; 69] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63,
        0x60, 0x60, 0x38, 0x01, 0x00, 0x00, 0xcc, 0x00, 0xc9, 0x27, 0x72, 0xc1, 0x5a, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
        0x42, 0x60, 0x82,
    ];

    /// The fixture MP3 with `picture` as its front cover, written to `path`.
    fn mp3_with_cover(path: &Path, picture: &[u8]) {
        let fixture = std::fs::read(fixture("sine.mp3")).unwrap();
        // Its own ID3v2 tag is a 10-byte header and 0x23 bytes of frames.
        let audio = &fixture[10 + 0x23..];
        let mut apic = vec![0];
        apic.extend_from_slice(b"image/png\0");
        apic.extend_from_slice(&[3, 0]);
        apic.extend_from_slice(picture);
        let mut frame = b"APIC".to_vec();
        frame.extend_from_slice(&(apic.len() as u32).to_be_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&apic);
        let size = frame.len() as u32;
        let synchsafe = [(size >> 21) as u8 & 0x7f, (size >> 14) as u8 & 0x7f, (size >> 7) as u8 & 0x7f, size as u8 & 0x7f];
        let tag = [b"ID3\x03\x00\x00".as_slice(), &synchsafe, &frame].concat();
        std::fs::write(path, [tag.as_slice(), audio].concat()).unwrap();
    }

    /// What the library knew of a song in iCloud before it was ever downloaded.
    fn placeholder(id: i64, path: &Path) -> Track {
        Track {
            path: path.to_string_lossy().into(),
            title: format!("{id:02} From Its Name"),
            in_cloud: true,
            duration_ms: 0,
            bitrate_kbps: None,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            ..track(id, "MP3", MediaKind::Music)
        }
    }

    /// A library, a cloud and the folders a sync works in, for the tests of `IpodSongs`.
    struct Bench {
        dir: tempfile::TempDir,
        library: LibraryState,
        covers: Covers,
        position: Cell<(u64, u64)>,
        events: RefCell<Vec<SyncProgress>>,
        conversions: HashMap<i64, (Conversion, bool)>,
        in_cloud: HashSet<i64>,
        cancel: AtomicBool,
    }

    impl Bench {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let library = LibraryState::open(dir.path().join("library.db"), dir.path().join("art")).unwrap();
            let covers = Covers::new(dir.path().join("covers"));
            Self {
                dir,
                library,
                covers,
                position: Cell::new((0, 0)),
                events: RefCell::default(),
                conversions: HashMap::new(),
                in_cloud: HashSet::new(),
                cancel: AtomicBool::new(false),
            }
        }

        /// A song in the library whose file is the fixture MP3 (with a cover, if given), down in
        /// the library as only in iCloud if `in_cloud`.
        fn song(&mut self, id: i64, in_cloud: bool, cover: Option<&[u8]>) -> SyncTrack {
            let path = self.dir.path().join(format!("{id}.mp3"));
            match cover {
                Some(picture) => mp3_with_cover(&path, picture),
                None => std::fs::copy(fixture("sine.mp3"), &path).map(drop).unwrap(),
            }
            let row = if in_cloud {
                placeholder(id, &path)
            } else {
                Track { path: path.to_string_lossy().into(), ..track(id, "MP3", MediaKind::Music) }
            };
            self.library.queries().import_tracks(std::slice::from_ref(&row)).unwrap();
            if in_cloud {
                self.in_cloud.insert(id);
            }
            sync_track(&row, Conversion::Copy)
        }

        fn songs<'a>(&'a self, cloud: &'a dyn Cloud, emit: &'a dyn Fn(SyncProgress), release: bool) -> IpodSongs<'a> {
            IpodSongs {
                library: &self.library,
                cloud,
                covers: &self.covers,
                emit,
                position: &self.position,
                conversions: &self.conversions,
                in_cloud: &self.in_cloud,
                convert_kbps: None,
                work_dir: self.dir.path(),
                cancel: &self.cancel,
                release,
                artwork: true,
                stall: Duration::from_secs(60),
                timeouts: 0,
                converted: None,
                downloaded: None,
                abandoned: Vec::new(),
                fetched: Vec::new(),
                skipped: Vec::new(),
            }
        }
    }

    fn cloud_with<'a>(songs: &[&SyncTrack]) -> FakeCloud<'a> {
        FakeCloud { placeholders: RefCell::new(songs.iter().map(|song| song.source_path.clone()).collect()), ..FakeCloud::default() }
    }

    #[test]
    fn a_song_that_is_here_is_copied_as_it_is_and_never_given_back() {
        let mut bench = Bench::new();
        let local = bench.song(1, false, None);
        let cloud = cloud_with(&[]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        let prepared = songs.prepare(&local);
        let fetched = songs.finish();

        assert_eq!(prepared, Prepared::File { path: local.source_path.clone(), track: None });
        assert!(cloud.downloads.borrow().is_empty() && cloud.released.borrow().is_empty());
        assert!(bench.events.borrow().is_empty());
        assert!(fetched.songs == 0 && fetched.skipped.is_empty() && fetched.unfinished.is_empty());
    }

    #[test]
    fn a_song_only_in_icloud_is_downloaded_described_by_its_file_and_given_back_once_copied() {
        let mut bench = Bench::new();
        let cloudy = bench.song(1, true, None);
        let local = bench.song(2, false, None);
        let cloud = cloud_with(&[&cloudy]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        bench.position.set((3, 10));

        let Prepared::File { path, track: Some(described) } = songs.prepare(&cloudy) else {
            panic!("expected the song and what its file says of it");
        };

        assert_eq!(path, cloudy.source_path);
        assert_eq!(*cloud.downloads.borrow(), std::slice::from_ref(&cloudy.source_path));
        let [event] = bench.events.borrow().clone().try_into().unwrap();
        assert_eq!(event, SyncProgress { stage: SyncStage::Downloading, done: 3, total: 10, current: "01 From Its Name".into() });
        // The placeholder had no length and a title from its name; the file has both.
        assert!(described.duration_ms > 0, "{}", described.duration_ms);
        assert_eq!(described.library_id, 1);
        let row = bench.library.queries().track(1).unwrap().unwrap();
        assert!(!row.in_cloud && row.duration_ms == described.duration_ms);
        // It is still being copied, so it is not given back yet.
        assert!(cloud.released.borrow().is_empty());

        // Asking for the next song means the last one was copied.
        songs.prepare(&local);
        assert_eq!(*cloud.released.borrow(), std::slice::from_ref(&cloudy.source_path));
        let fetched = songs.finish();
        assert_eq!(fetched.songs, 1);
        assert_eq!(cloud.released.borrow().len(), 1, "a song that was already here is never given back");
    }

    #[test]
    fn a_song_the_library_has_as_in_icloud_but_that_is_here_is_read_and_never_given_back() {
        let mut bench = Bench::new();
        let downloaded_since = bench.song(1, true, None);
        let cloud = cloud_with(&[]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        let Prepared::File { track: Some(described), .. } = songs.prepare(&downloaded_since) else {
            panic!("expected the song as its file describes it");
        };
        songs.finish();

        assert!(described.duration_ms > 0);
        assert!(cloud.downloads.borrow().is_empty() && cloud.released.borrow().is_empty());
        assert!(bench.events.borrow().is_empty(), "nothing was downloaded to announce");
        assert!(!bench.library.queries().track(1).unwrap().unwrap().in_cloud);
    }

    #[test]
    fn the_last_song_is_given_back_when_the_sync_ends_and_none_are_when_the_setting_is_off() {
        for (release, given_back) in [(true, 1), (false, 0)] {
            let mut bench = Bench::new();
            let cloudy = bench.song(1, true, None);
            let cloud = cloud_with(&[&cloudy]);
            let emit = |progress| bench.events.borrow_mut().push(progress);
            let mut songs = bench.songs(&cloud, &emit, release);

            songs.prepare(&cloudy);
            songs.finish();

            assert_eq!(cloud.released.borrow().len(), given_back);
        }
    }

    #[test]
    fn a_download_that_fails_leaves_the_song_out_and_the_sync_goes_on() {
        let mut bench = Bench::new();
        let broken = bench.song(1, true, None);
        let fine = bench.song(2, true, None);
        let mut cloud = cloud_with(&[&broken, &fine]);
        cloud.failing.insert(broken.source_path.clone());
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        assert_eq!(songs.prepare(&broken), Prepared::Skip);
        assert!(matches!(songs.prepare(&fine), Prepared::File { track: Some(_), .. }));
        let fetched = songs.finish();

        assert_eq!(fetched.skipped, ["couldn't be downloaded (i/o error: offline)"]);
        assert!(fetched.unfinished.is_empty(), "a download that failed has no request left to answer");
        assert_eq!(fetched.songs, 1);
        assert!(bench.library.queries().track(1).unwrap().unwrap().in_cloud, "the library still says it is in iCloud");
    }

    #[test]
    fn three_downloads_in_a_row_that_stall_mean_the_rest_are_not_tried() {
        let mut bench = Bench::new();
        let stuck: Vec<SyncTrack> = (1..=4).map(|id| bench.song(id, true, None)).collect();
        let after = bench.song(5, false, None);
        let cloud = FakeCloud { slow: Some(Duration::MAX), ..cloud_with(&stuck.iter().collect::<Vec<_>>()) };
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        songs.stall = Duration::from_millis(20);

        for song in &stuck {
            assert_eq!(songs.prepare(song), Prepared::Skip);
        }
        // A song that is already here needs no iCloud.
        assert!(matches!(songs.prepare(&after), Prepared::File { .. }));

        assert_eq!(cloud.downloads.borrow().len(), 3, "the fourth was not even asked for");
        assert_eq!(songs.finish().skipped, [TIMED_OUT, TIMED_OUT, TIMED_OUT, NO_RESPONSE]);
    }

    #[test]
    fn a_download_that_gets_through_starts_the_count_of_stalls_again() {
        let mut bench = Bench::new();
        let songs_in_cloud: Vec<SyncTrack> = (1..=6).map(|id| bench.song(id, true, None)).collect();
        let mut cloud = cloud_with(&songs_in_cloud.iter().collect::<Vec<_>>());
        // Two stall, one comes through, two stall: five tries in all, never three in a row.
        for index in [0, 1, 3, 4] {
            cloud.hanging.insert(songs_in_cloud[index].source_path.clone());
        }
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        songs.stall = Duration::from_millis(20);

        let outcomes: Vec<bool> = songs_in_cloud[..5].iter().map(|song| matches!(songs.prepare(song), Prepared::File { .. })).collect();

        assert_eq!(outcomes, [false, false, true, false, false]);
        assert_eq!(cloud.downloads.borrow().len(), 5);
        assert!(!songs.finish().skipped.contains(&NO_RESPONSE.to_owned()));
    }

    #[test]
    fn a_big_file_is_given_longer_before_it_counts_as_stalled() {
        assert_eq!(stall_limit(Duration::from_secs(180), 10 << 20), Duration::from_secs(180));
        assert_eq!(stall_limit(Duration::from_secs(180), 1 << 30), Duration::from_secs(4096));

        let mut bench = Bench::new();
        let book = bench.song(1, true, None);
        // Nothing arrives for far longer than the floor, but the file is a gigabyte.
        let cloud = FakeCloud { slow: Some(Duration::from_millis(80)), total: Some(1 << 40), ..cloud_with(&[&book]) };
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        songs.stall = Duration::from_millis(20);

        assert!(matches!(songs.prepare(&book), Prepared::File { .. }));
        assert!(songs.finish().skipped.is_empty());
    }

    #[test]
    fn stopping_during_a_download_ends_the_sync_and_is_not_counted_as_a_song_left_out() {
        let mut bench = Bench::new();
        let cloudy = bench.song(1, true, None);
        let cloud = FakeCloud { slow: Some(Duration::MAX), cancels: Some(&bench.cancel), ..cloud_with(&[&cloudy]) };
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        let prepared = songs.prepare(&cloudy);
        let fetched = songs.finish();

        assert_eq!(prepared, Prepared::Stop);
        assert!(fetched.skipped.is_empty());
        // iCloud was asked for it and may yet send it: it is noted, to be given back when it does.
        assert_eq!(fetched.unfinished, std::slice::from_ref(&cloudy.source_path));
        assert!(cloud.released.borrow().is_empty(), "still a placeholder, so nothing to give back yet");
    }

    #[test]
    fn a_sync_that_is_stopped_before_a_song_is_asked_for_stops() {
        let mut bench = Bench::new();
        let local = bench.song(1, false, None);
        bench.cancel.store(true, Ordering::Relaxed);
        let cloud = cloud_with(&[]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        assert_eq!(songs.prepare(&local), Prepared::Stop);
    }

    #[test]
    fn a_download_given_up_on_that_finishes_later_is_given_back_when_the_sync_ends() {
        let mut bench = Bench::new();
        let (late, never) = (bench.song(1, true, None), bench.song(2, true, None));
        let cloud = FakeCloud { slow: Some(Duration::MAX), ..cloud_with(&[&late, &never]) };
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        songs.stall = Duration::from_millis(20);
        assert_eq!(songs.prepare(&late), Prepared::Skip);
        // The offline Mac gets its connection back, and iCloud answers the request.
        cloud.placeholders.borrow_mut().remove(&late.source_path);
        cloud.placeholders.borrow_mut().insert(never.source_path.clone());
        songs.abandoned.push(never.source_path.clone());

        let fetched = songs.finish();

        assert_eq!(*cloud.released.borrow(), std::slice::from_ref(&late.source_path));
        assert_eq!(fetched.unfinished, std::slice::from_ref(&never.source_path));
    }

    #[test]
    fn with_the_setting_off_an_abandoned_download_is_nobodys_to_give_back() {
        let mut bench = Bench::new();
        let stuck = bench.song(1, true, None);
        let cloud = FakeCloud { slow: Some(Duration::MAX), ..cloud_with(&[&stuck]) };
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, false);
        songs.stall = Duration::from_millis(20);

        songs.prepare(&stuck);

        assert!(songs.finish().unfinished.is_empty());
    }

    #[test]
    fn a_download_that_cannot_be_read_is_left_out() {
        let mut bench = Bench::new();
        let broken = bench.song(1, true, None);
        std::fs::write(&broken.source_path, b"this is not audio").unwrap();
        let cloud = cloud_with(&[&broken]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        assert_eq!(songs.prepare(&broken), Prepared::Skip);

        assert_eq!(songs.finish().skipped, [UNREADABLE]);
    }

    #[test]
    fn a_song_that_will_not_convert_is_left_out_and_the_sync_goes_on() {
        let mut bench = Bench::new();
        let song = bench.song(1, false, None);
        std::fs::write(&song.source_path, b"this is not audio").unwrap();
        bench.conversions.insert(1, (Conversion::Alac, true));
        let cloud = cloud_with(&[]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        assert_eq!(songs.prepare(&song), Prepared::Skip);
        assert_eq!(songs.finish().skipped, [NOT_CONVERTED]);
    }

    #[test]
    fn a_downloaded_songs_cover_is_kept_as_a_jpeg_from_the_moment_it_is_here() {
        let mut bench = Bench::new();
        let (one, two, three) = (bench.song(1, true, Some(&RED)), bench.song(2, true, Some(&RED)), bench.song(3, true, Some(&BLUE)));
        let cloud = cloud_with(&[&one, &two, &three]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);

        for song in [&one, &two, &three] {
            songs.prepare(song);
        }
        songs.finish();

        let kept = |id| bench.covers.get(id).unwrap();
        assert!(kept(1).starts_with(&[0xff, 0xd8]), "shrunk into a JPEG, not the picture as it was embedded");
        assert_eq!(kept(1), kept(2));
        assert_ne!(kept(1), kept(3));
        assert_eq!(bench.covers.get(4), None);
        assert_eq!(std::fs::read_dir(bench.dir.path().join("covers")).unwrap().count(), 2, "one file per distinct picture");
        bench.covers.clear();
        assert_eq!(bench.covers.get(1), None);
        let cached = std::fs::read_dir(bench.dir.path().join("art")).map_or(0, Iterator::count);
        assert_eq!(cached, 0, "no thumbnail is left in the library's cache for a song that goes back to iCloud");
    }

    #[test]
    fn no_cover_is_kept_for_an_ipod_that_shows_no_album_art() {
        let mut bench = Bench::new();
        let song = bench.song(1, true, Some(&RED));
        let cloud = cloud_with(&[&song]);
        let emit = |progress| bench.events.borrow_mut().push(progress);
        let mut songs = bench.songs(&cloud, &emit, true);
        songs.artwork = false;

        assert!(matches!(songs.prepare(&song), Prepared::File { .. }));

        assert_eq!(bench.covers.get(1), None);
        assert!(!bench.dir.path().join("covers").exists());
    }

    #[test]
    fn downloads_left_unfinished_are_given_back_once_they_finish_and_forgotten_once_gone() {
        let dir = tempfile::tempdir().unwrap();
        let (done, waiting, gone) = (dir.path().join("done.mp3"), dir.path().join("waiting.mp3"), dir.path().join("gone.mp3"));
        std::fs::write(&done, b"x").unwrap();
        std::fs::write(&waiting, b"x").unwrap();
        let leftovers = Leftovers { file: dir.path().join("cache").join("leftovers.json") };
        let always = |_: &str| true;
        leftovers.add(&[done.clone(), waiting.clone(), gone], "ipod", 1_000);
        leftovers.add(std::slice::from_ref(&waiting), "ipod", 1_010);
        assert_eq!(leftovers.load(1_020).len(), 3, "a path is listed once");
        let cloud = FakeCloud { placeholders: RefCell::new(HashSet::from([waiting.clone()])), ..FakeCloud::default() };

        leftovers.release(&cloud, &always, 1_030);

        assert_eq!(*cloud.released.borrow(), [done]);
        let kept = leftovers.load(1_030);
        assert_eq!(kept.iter().map(|left| &left.path).collect::<Vec<_>>(), [&waiting]);
        // Nothing more has finished, so another go does nothing.
        leftovers.release(&cloud, &always, 1_040);
        assert_eq!(cloud.released.borrow().len(), 1);
        cloud.placeholders.borrow_mut().clear();
        leftovers.release(&cloud, &always, 1_050);
        assert_eq!(cloud.released.borrow().len(), 2);
        assert!(!leftovers.file.exists(), "an empty list is no file");
    }

    #[test]
    fn a_leftover_is_given_back_only_where_its_ipods_setting_says_so_and_only_for_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let (on, off, old) = (dir.path().join("on.mp3"), dir.path().join("off.mp3"), dir.path().join("old.mp3"));
        for file in [&on, &off, &old] {
            std::fs::write(file, b"x").unwrap();
        }
        let leftovers = Leftovers { file: dir.path().join("leftovers.json") };
        leftovers.add(std::slice::from_ref(&old), "on", 0);
        leftovers.add(std::slice::from_ref(&on), "on", 100);
        leftovers.add(std::slice::from_ref(&off), "off", 100);
        let cloud = FakeCloud::default();

        leftovers.release(&cloud, &|device: &str| device == "on", LEFTOVER_LIFE_SECS + 50);

        assert_eq!(
            *cloud.released.borrow(),
            std::slice::from_ref(&on),
            "not the other iPod's, and not the one nobody has asked about in a day"
        );
        assert!(leftovers.load(LEFTOVER_LIFE_SECS + 50).is_empty());
    }

    #[test]
    fn a_song_that_is_played_is_taken_off_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let (played, other) = (dir.path().join("played.mp3"), dir.path().join("other.mp3"));
        let leftovers = Leftovers { file: dir.path().join("leftovers.json") };
        leftovers.add(&[played.clone(), other.clone()], "ipod", 5);

        leftovers.forget(&played, 6);

        let kept = leftovers.load(6);
        assert_eq!(kept.iter().map(|left| &left.path).collect::<Vec<_>>(), [&other]);
        leftovers.forget(&other, 7);
        assert!(!leftovers.file.exists());
    }

    #[test]
    fn a_sync_or_copy_holds_its_device_and_is_the_only_one() {
        let dir = tempfile::tempdir().unwrap();
        let state = SyncState::load(dir.path().join("DeviceSync.json"));
        assert!(!state.is_busy_with("ipod"));

        let cancel = state.begin("ipod").unwrap();

        assert!(state.is_busy_with("ipod") && !state.is_busy_with("another"));
        assert_eq!(state.begin("another").unwrap_err().to_string(), "Another sync or copy is still running.");
        assert!(!cancel.load(Ordering::Relaxed));
        state.finish();
        assert!(!state.is_busy_with("ipod"));
        assert!(state.begin("another").is_ok());
    }

    #[test]
    fn songs_left_out_are_reported_by_how_many_and_why() {
        let reasons: Vec<String> = [TIMED_OUT, UNREADABLE, TIMED_OUT, NO_RESPONSE].map(String::from).into();
        assert_eq!(
            unsynced_notice(&reasons),
            "Synced, but 4 songs weren't copied: 1 couldn't be read once downloaded; 1 skipped because iCloud isn't responding; 2 timed out waiting for iCloud"
        );
        assert_eq!(unsynced_notice(&reasons[..1]), "Synced, but 1 song wasn't copied: 1 timed out waiting for iCloud");
    }
}
