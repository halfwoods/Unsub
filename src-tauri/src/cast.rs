//! Playing to speakers: Chromecasts, Sonos and other UPnP renderers on the local network. The
//! speaker is given a link and fetches the song itself, so while one is chosen the songs are served
//! to it from a small web server of its own, whether or not library sharing is on.
//!
//! A chosen speaker takes the audio engine's place for as long as it is chosen. The play queue,
//! play counts and the rest of `playback.rs` carry on as they are: the speaker's driver reports
//! what happens as the same player events the engine does, so the queue moves on when a song ends
//! and the LCD follows the speaker's position.

use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use unsub_cast::{
    AudioFormat, Browser, Codec, Command, Device, Driver, Error as CastError, Event, Handle, Kind, Links, MAX_DEPTH, MAX_RATE, Media,
    Provider, Snapshot, State as Speaker,
};
use unsub_library::Track;
use unsub_playback::{Decoder, PlaybackState as EngineState, PlaybackStatus, PlayerEvent, Target, TranscodeOptions};
use unsub_remote::Song;
use unsub_subsonic::{LinkSource, Server, Stream};

use crate::devices::DevicesState;
use crate::error::{CommandError, CommandResult};
use crate::library::LibraryState;
use crate::playback::{PlaySource, PlaybackState, Tracks};
use crate::remote::RemoteState;

/// Carries the speaker the music is playing on, or `null` when it is this computer.
pub const TARGET_EVENT: &str = "cast://target";

/// Cover art goes to speakers at this size: they show it small, and the file is fetched over Wi-Fi.
const ART_SIZE: u32 = 600;

/// Songs the provider remembers how to prepare. The driver only ever asks about the song it is
/// playing and the one queued, both registered a moment before, so a handful is plenty.
const REMEMBERED_SONGS: usize = 8;

/// The System Settings pane that lists outputs, AirPlay speakers among them. The old pane name is
/// the one every macOS since 11 understands, the newest by mapping it to its settings extension.
#[cfg(target_os = "macos")]
const SOUND_SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.sound";

/// What a link opens.
#[derive(Clone)]
enum Served {
    File { path: PathBuf, mime: String },
    Bytes { bytes: Arc<[u8]>, mime: String },
}

struct Open(Arc<Links<Served>>);

impl LinkSource for Open {
    fn open(&self, secret: &str) -> Option<Stream> {
        Some(match self.0.open(secret)? {
            Served::File { path, mime } => Stream::File { path, content_type: mime },
            Served::Bytes { bytes, mime } => {
                let length = bytes.len() as u64;
                Stream::Reader { reader: Box::new(Cursor::new(bytes)), content_type: mime, length: Some(length) }
            }
        })
    }
}

/// Where a song's cover art is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ArtSource {
    Library(i64),
    Device { device_id: String, track_id: i64 },
}

/// A song as far as the speaker is concerned: where its file is, what to show, what is in it.
#[derive(Clone, Debug)]
struct Item {
    path: PathBuf,
    title: String,
    artist: String,
    album: String,
    duration_ms: u64,
    /// What the library, the iPod or the server says is in the file. The file itself is asked
    /// before this is trusted, and this is for when it cannot be read.
    format: Option<AudioFormat>,
    art: Option<ArtSource>,
}

impl Item {
    fn of_track(track: &Track, path: &Path, art: Option<ArtSource>) -> Self {
        Self {
            path: path.to_owned(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration_ms: track.duration_ms,
            format: Some(AudioFormat { codec: Codec::named(&track.codec), sample_rate: track.sample_rate, bit_depth: track.bit_depth }),
            art,
        }
    }

    /// A song from a shared library, which says little of its file.
    fn of_song(song: &Song, path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            title: song.title.clone(),
            artist: song.artist.clone().unwrap_or_default(),
            album: song.album.clone().unwrap_or_default(),
            duration_ms: song.duration.map_or(0, |seconds| u64::from(seconds) * 1000),
            format: None,
            art: None,
        }
    }

    /// A song of which only the file is to hand.
    fn of_file(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            title: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            artist: String::new(),
            album: String::new(),
            duration_ms: 0,
            format: None,
            art: None,
        }
    }
}

/// What the song is called and where it comes from, however it got into the queue.
fn describe(app: &AppHandle, source: &PlaySource, track_id: i64, path: &Path) -> Item {
    let found = match source {
        PlaySource::Library => app
            .state::<LibraryState>()
            .queries()
            .track(track_id)
            .ok()
            .flatten()
            .map(|track| Item::of_track(&track, path, Some(ArtSource::Library(track_id)))),
        PlaySource::Device { device_id } => app
            .state::<DevicesState>()
            .track(device_id, track_id)
            .map(|track| Item::of_track(&track, path, Some(ArtSource::Device { device_id: device_id.clone(), track_id }))),
        PlaySource::Cd { disc_id } => {
            app.state::<DevicesState>().cds.track(disc_id, track_id).map(|track| Item::of_track(&track, path, None))
        }
        PlaySource::Shared { library } => app.state::<RemoteState>().song_info(library, track_id).map(|song| Item::of_song(&song, path)),
    };
    found.unwrap_or_else(|| Item::of_file(path))
}

/// What is really in the file, read from the file. `None` if it can't be opened.
fn probe(path: &Path) -> Option<AudioFormat> {
    let decoder = Decoder::open(path).ok()?;
    let codec = match decoder.codec() {
        unsub_playback::Codec::Mp3 => Codec::Mp3,
        unsub_playback::Codec::Aac => Codec::Aac,
        unsub_playback::Codec::Alac => Codec::Alac,
        unsub_playback::Codec::Flac => Codec::Flac,
        unsub_playback::Codec::Vorbis => Codec::Vorbis,
        unsub_playback::Codec::Opus => Codec::Opus,
        unsub_playback::Codec::Pcm => match decoder.container() {
            "wav" | "wave" => Codec::Wav,
            "aiff" => Codec::Aiff,
            _ => Codec::Other,
        },
        _ => Codec::Other,
    };
    Some(AudioFormat {
        codec,
        sample_rate: Some(decoder.sample_rate()),
        bit_depth: decoder.bit_depth().and_then(|bits| u8::try_from(bits).ok()),
    })
}

/// The folder a session converts songs into. Each session has its own, so that ending one never
/// takes the songs of the one that replaced it.
fn session_dir(cache_dir: &Path, session: u64) -> PathBuf {
    cache_dir.join(format!("session-{session}"))
}

/// What a link's address holds: `…/link/<secret>.<extension>`.
fn secret_of(url: &str) -> Option<&str> {
    url.rsplit_once("/link/")?.1.split('.').next()
}

fn core_error(message: impl Into<String>) -> CastError {
    CastError::Refused(message.into())
}

/// Finds a song's cover art as (content type, bytes), at a size for speakers.
type ArtworkOf = Box<dyn Fn(&ArtSource) -> Option<(String, Vec<u8>)> + Send + Sync>;

/// Makes songs available to one speaker. Songs it can play are served as they are, the others are
/// converted to FLAC first, into files that are deleted as soon as the speaker is done with them.
struct Songs {
    kind: Kind,
    speaker: String,
    /// `http://<this computer's address toward the speaker>:<port>`.
    base: String,
    /// Where converted songs are written.
    dir: PathBuf,
    links: Arc<Links<Served>>,
    items: Mutex<VecDeque<(i64, Item)>>,
    /// Converted files, by the secret of the link that serves them.
    converted: Mutex<HashMap<String, PathBuf>>,
    art: ArtworkOf,
    events: Sender<PlayerEvent>,
    conversions: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Songs {
    fn register(&self, id: i64, item: Item) {
        let mut items = lock(&self.items);
        items.retain(|(known, _)| *known != id);
        items.push_back((id, item));
        while items.len() > REMEMBERED_SONGS {
            items.pop_front();
        }
    }

    fn item(&self, id: i64) -> Option<Item> {
        lock(&self.items).iter().find(|(known, _)| *known == id).map(|(_, item)| item.clone())
    }

    fn url(&self, secret: &str, extension: &str) -> String {
        format!("{}/link/{secret}.{extension}", self.base)
    }

    fn convert(&self, item: &Item, id: i64, cancel: &AtomicBool) -> Result<PathBuf, CastError> {
        let _ = self.events.send(PlayerEvent::Notice { message: format!("Converting “{}” for {}…", item.title, self.speaker) });
        let output = self.dir.join(format!("{id}-{}.flac", self.conversions.fetch_add(1, Ordering::Relaxed)));
        let options =
            TranscodeOptions { target: Target::Flac, max_sample_rate: Some(MAX_RATE), max_bit_depth: Some(MAX_DEPTH), keep_tags: false };
        unsub_playback::transcode_file(&item.path, &output, &options, &mut |_, _| {}, cancel)
            .map_err(|error| core_error(format!("couldn’t convert it for the speaker: {error}")))?;
        Ok(output)
    }

    fn art_url(&self, source: &ArtSource) -> Option<String> {
        let (mime, bytes) = (self.art)(source)?;
        let extension = if mime.contains("png") { "png" } else { "jpg" };
        let secret = self.links.mint(Served::Bytes { bytes: bytes.into(), mime }).ok()?;
        Some(self.url(&secret, extension))
    }
}

impl Provider for Songs {
    fn media(&self, id: i64, cancel: &AtomicBool) -> Result<Media, CastError> {
        let item = self.item(id).ok_or_else(|| core_error("that song is no longer queued"))?;
        // The file's own word on what is in it, since a 24-bit 96 kHz file from a shared library or
        // an iPod would otherwise be sent to a speaker that can't play it.
        let playable = probe(&item.path).or(item.format).filter(|&format| self.kind.plays(format));
        let (path, codec, converted) = match playable {
            Some(format) => (item.path.clone(), format.codec, false),
            None => (self.convert(&item, id, cancel)?, Codec::Flac, true),
        };
        let secret = self.links.mint(Served::File { path: path.clone(), mime: codec.mime().to_owned() })?;
        if converted {
            lock(&self.converted).insert(secret.clone(), path);
        }
        Ok(Media {
            url: self.url(&secret, codec.extension()),
            mime: codec.mime().to_owned(),
            title: item.title,
            artist: item.artist,
            album: item.album,
            art_url: item.art.as_ref().and_then(|source| self.art_url(source)),
            duration_ms: item.duration_ms,
        })
    }

    fn serves(&self, url: &str) -> bool {
        url.strip_prefix(&self.base).is_some_and(|rest| rest.starts_with("/link/"))
    }

    fn release(&self, media: &Media) {
        for url in std::iter::once(&media.url).chain(&media.art_url) {
            let Some(secret) = secret_of(url) else { continue };
            self.links.close(secret);
            if let Some(file) = lock(&self.converted).remove(secret) {
                let _ = std::fs::remove_file(file);
            }
        }
    }

    fn keep(&self, media: &Media) {
        for url in std::iter::once(&media.url).chain(&media.art_url) {
            if let Some(secret) = secret_of(url) {
                self.links.keep(secret);
            }
        }
    }
}

/// How the engine's status reads for a song on a speaker.
fn status_of(snapshot: &Snapshot, fallback_volume: f32) -> PlaybackStatus {
    PlaybackStatus {
        state: match snapshot.state {
            Speaker::Stopped => EngineState::Stopped,
            Speaker::Paused => EngineState::Paused,
            Speaker::Playing | Speaker::Buffering => EngineState::Playing,
        },
        track_id: snapshot.id,
        position_ms: snapshot.position_ms,
        duration_ms: snapshot.duration_ms,
        sample_rate: None,
        output_sample_rate: None,
        volume: snapshot.volume.unwrap_or(fallback_volume),
        bit_perfect: false,
        output_format: None,
        gain_db: None,
        eq_active: false,
        live: false,
        buffering: false,
    }
}

/// What the app hears of the speaker. A lost speaker is not an event here: it ends the session.
fn player_event(event: &Event, speaker: &str, fallback_volume: f32) -> Option<PlayerEvent> {
    Some(match event {
        Event::Started(track_id) => PlayerEvent::TrackStarted { track_id: *track_id },
        Event::Ended(track_id) => PlayerEvent::TrackEnded { track_id: *track_id },
        Event::Position { id, position_ms, duration_ms } => {
            PlayerEvent::Position { track_id: *id, position_ms: *position_ms, duration_ms: *duration_ms }
        }
        Event::State(snapshot) => PlayerEvent::StateChanged(status_of(snapshot, fallback_volume)),
        Event::Failed { message, .. } => PlayerEvent::Notice { message: format!("{speaker} couldn’t play the song: {message}") },
        Event::Lost(_) => return None,
    })
}

/// A speaker, connected, with the web server that feeds it.
struct Session {
    /// Tells this session from the one before or after it, for a lost speaker's report to find its own.
    id: u64,
    device: Device,
    handle: Handle,
    songs: Arc<Songs>,
    server: Server,
    dir: PathBuf,
    /// What the volume reads until the speaker says what it is.
    fallback_volume: f32,
}

impl Session {
    /// Stops the speaker and everything serving it. Says where it was.
    fn end(self) -> Snapshot {
        let last = self.handle.shutdown();
        self.songs.links.close_all();
        self.server.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
        last
    }
}

pub struct CastState {
    events: Sender<PlayerEvent>,
    /// Whether a speaker has taken over from the engine. Shared with the engine's event callback,
    /// which drops what the engine says meanwhile: it is stopped, and its stopping is not news.
    casting: Arc<AtomicBool>,
    /// Where songs are converted, a folder per session.
    cache_dir: PathBuf,
    devices: Mutex<Vec<Device>>,
    browser: Mutex<Option<Browser>>,
    session: Mutex<Option<Session>>,
    sessions: AtomicU64,
    /// Held while the output is switched or a lost speaker is let go of. Taken before `playing`.
    switching: Mutex<()>,
}

impl CastState {
    pub fn new(events: Sender<PlayerEvent>, casting: Arc<AtomicBool>, cache_dir: PathBuf) -> Self {
        // A session that never got to tidy up, such as one the app was killed during, leaves its songs here.
        let _ = std::fs::remove_dir_all(&cache_dir);
        Self {
            events,
            casting,
            cache_dir,
            devices: Mutex::default(),
            browser: Mutex::default(),
            session: Mutex::default(),
            sessions: AtomicU64::new(0),
            switching: Mutex::default(),
        }
    }

    fn session(&self) -> MutexGuard<'_, Option<Session>> {
        lock(&self.session)
    }

    pub fn is_casting(&self) -> bool {
        self.casting.load(Ordering::Relaxed)
    }

    fn notice(&self, message: String) {
        let _ = self.events.send(PlayerEvent::Notice { message });
    }

    fn device(&self, id: &str) -> Option<Device> {
        lock(&self.devices).iter().find(|device| device.id == id).cloned()
    }

    fn target(&self) -> Option<Device> {
        self.session().as_ref().map(|session| session.device.clone())
    }

    /// Hands a song to the speaker, if one is chosen. Whether one took it.
    pub fn play(&self, app: &AppHandle, source: &PlaySource, track_id: i64, path: &Path, start_ms: u64, paused: bool) -> bool {
        if !self.is_casting() {
            return false;
        }
        let item = describe(app, source, track_id, path);
        let session = self.session();
        let Some(session) = session.as_ref() else { return false };
        session.songs.register(track_id, item);
        session.handle.send(Command::Play { id: track_id, start_ms, paused });
        true
    }

    /// Has the speaker play the song after the current one, if a speaker is chosen.
    pub fn queue(&self, app: &AppHandle, source: &PlaySource, track_id: i64, path: &Path) -> bool {
        if !self.is_casting() {
            return false;
        }
        let item = describe(app, source, track_id, path);
        let session = self.session();
        let Some(session) = session.as_ref() else { return false };
        session.songs.register(track_id, item);
        session.handle.send(Command::Queue { id: track_id });
        true
    }

    /// Sends a command to the speaker, if one is chosen. Whether there was one.
    pub fn command(&self, command: Command) -> bool {
        let session = self.session();
        session.as_ref().map(|session| session.handle.send(command)).is_some()
    }

    pub fn pause(&self) -> bool {
        self.command(Command::Pause)
    }

    pub fn resume(&self) -> bool {
        self.command(Command::Resume)
    }

    pub fn seek(&self, position_ms: u64) -> bool {
        self.command(Command::Seek(position_ms))
    }

    pub fn set_volume(&self, volume: f32) -> bool {
        self.command(Command::SetVolume(volume))
    }

    pub fn stop(&self) -> bool {
        self.command(Command::Stop)
    }

    pub fn clear_queue(&self) -> bool {
        self.command(Command::ClearQueue)
    }

    pub fn status(&self) -> Option<PlaybackStatus> {
        let session = self.session();
        let session = session.as_ref()?;
        Some(status_of(&session.handle.snapshot(), session.fallback_volume))
    }

    /// Ends session `id`, or whichever there is if `None`, and says where the speaker had got to.
    fn take(&self, id: Option<u64>) -> Option<(Snapshot, f32)> {
        let session = {
            let mut slot = self.session();
            if id.is_some_and(|id| slot.as_ref().is_none_or(|session| session.id != id)) {
                return None;
            }
            self.casting.store(false, Ordering::Relaxed);
            slot.take()?
        };
        let fallback = session.fallback_volume;
        Some((session.end(), fallback))
    }

    /// Stops playing to the speaker and returns to the computer, with a notice saying why.
    pub fn release(&self, app: &AppHandle, why: &str) {
        if self.take(None).is_some() {
            let _ = app.emit(TARGET_EVENT, None::<Device>);
            self.notice(why.to_owned());
        }
    }

    /// For the app quitting: stops the speaker rather than leaving it playing to a server that is gone.
    pub fn end(&self) {
        self.take(None);
        *lock(&self.browser) = None;
    }

    /// Looks for speakers again, rather than at the next scheduled search.
    fn search_again(&self) {
        if let Some(browser) = lock(&self.browser).as_ref() {
            browser.search_now();
        }
    }

    /// Connects to `device` and starts serving songs to it.
    fn open(&self, app: &AppHandle, device: Device) -> CommandResult<Session> {
        let name = device.name.clone();
        let address = unsub_cast::local_address_toward(device.address)
            .ok_or_else(|| CommandError::from(format!("This computer has no network connection that reaches {name}.")))?;
        let receiver = unsub_cast::connect(&device).map_err(|error| CommandError::from(format!("Couldn’t connect to {name}: {error}")))?;

        let links: Arc<Links<Served>> = Arc::default();
        // Only the interface that reaches the speaker listens, and only while it is chosen.
        let server = Server::start_links(SocketAddr::new(address, 0), Arc::new(Open(Arc::clone(&links))))
            .map_err(|error| CommandError::from(format!("Couldn’t start serving songs to {name}: {error}")))?;
        let id = self.sessions.fetch_add(1, Ordering::Relaxed);
        let dir = session_dir(&self.cache_dir, id);
        std::fs::create_dir_all(&dir)
            .map_err(|error| CommandError::from(format!("Couldn’t make room to convert songs for {name}: {error}")))?;
        let library = app.clone();
        let songs = Arc::new(Songs {
            kind: device.kind,
            speaker: name.clone(),
            base: format!("http://{}", server.local_addr()),
            dir: dir.clone(),
            links,
            items: Mutex::default(),
            converted: Mutex::default(),
            art: Box::new(move |source| {
                match source {
                    ArtSource::Library(track_id) => library.state::<LibraryState>().artwork(*track_id, Some(ART_SIZE)),
                    ArtSource::Device { device_id, track_id } => library.state::<DevicesState>().artwork(device_id, *track_id),
                }
                .map(|art| (art.mime, art.bytes))
            }),
            events: self.events.clone(),
            conversions: AtomicU64::new(0),
        });

        let events = self.events.clone();
        let app = app.clone();
        let fallback_volume = app.state::<PlaybackState>().player.status().volume;
        let speaker = name.clone();
        let driver = Driver::new(receiver, songs.clone(), move |event| {
            if let Event::Lost(reason) = &event {
                // Ending the session joins this driver's thread, so it can't be done from it.
                let (app, speaker, reason) = (app.clone(), speaker.clone(), reason.clone());
                std::thread::spawn(move || lost(&app, id, &speaker, &reason));
            } else if let Some(event) = player_event(&event, &speaker, fallback_volume) {
                let _ = events.send(event);
            }
        });
        let handle = unsub_cast::spawn(driver).map_err(|error| CommandError::from(format!("Couldn’t start playing to {name}: {error}")))?;
        Ok(Session { id, device, handle, songs, server, dir, fallback_volume })
    }
}

/// The speaker has gone: back to the computer, stopped.
fn lost(app: &AppHandle, session: u64, speaker: &str, reason: &str) {
    let cast = app.state::<CastState>();
    let _switching = lock(&cast.switching);
    // A switch to another speaker may have got there first.
    if cast.take(Some(session)).is_none() {
        return;
    }
    let _ = app.emit(TARGET_EVENT, None::<Device>);
    // Playing here again, the volume slider is the engine's to say.
    let stopped = PlaybackStatus {
        state: EngineState::Stopped,
        track_id: None,
        position_ms: 0,
        duration_ms: 0,
        ..app.state::<PlaybackState>().player.status()
    };
    let _ = cast.events.send(PlayerEvent::StateChanged(stopped));
    cast.notice(format!("Stopped playing to {speaker}: {reason}."));
}

/// Starts looking for speakers, and keeps the list.
pub fn browse(app: &AppHandle) {
    let handle = app.clone();
    let found = unsub_cast::browse(move |devices| *lock(&handle.state::<CastState>().devices) = devices);
    match found {
        Ok(browser) => *lock(&app.state::<CastState>().browser) = Some(browser),
        // Without discovery the speaker menu is just empty; the music still plays here.
        Err(error) => eprintln!("[cast] can't look for speakers: {error}"),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Speakers {
    devices: Vec<Device>,
    target: Option<Device>,
    /// The Mac's own output is an AirPlay speaker.
    airplay: bool,
}

/// The speakers to offer in the menu, and which one is playing. Also looks again, for next time.
#[tauri::command]
pub fn cast_speakers(state: State<'_, CastState>) -> Speakers {
    state.search_again();
    Speakers { devices: lock(&state.devices).clone(), target: state.target(), airplay: unsub_playback::output_is_airplay() }
}

/// Plays on the speaker `id`, or on the computer with `None`. The song playing carries on from where it is.
#[tauri::command]
pub async fn cast_select(app: AppHandle, id: Option<String>) -> CommandResult<()> {
    // Connecting can take seconds, and the IPC thread has the window to serve.
    tauri::async_runtime::spawn_blocking(move || select(&app, id.as_deref()))
        .await
        .map_err(|error| CommandError::from(error.to_string()))?
}

fn select(app: &AppHandle, id: Option<&str>) -> CommandResult<()> {
    let cast = app.state::<CastState>();
    let _switching = lock(&cast.switching);
    let device = match id {
        Some(id) => Some(cast.device(id).ok_or_else(|| CommandError::from("That speaker isn’t available any more.".to_owned()))?),
        None => None,
    };
    if cast.target().map(|d| d.id) == device.as_ref().map(|d| d.id.clone()) {
        return Ok(());
    }
    let playback = app.state::<PlaybackState>();
    if device.is_some() && playback.player.status().live {
        return Err(CommandError::from("Stations play on this computer.".to_owned()));
    }
    // Connected before anything is stopped, so a speaker that can't be reached leaves the music playing where it was.
    let opened = match &device {
        Some(device) => {
            cast.notice(format!("Connecting to {}…", device.name));
            Some(cast.open(app, device.clone())?)
        }
        None => None,
    };

    let library = app.state::<LibraryState>();
    let devices = app.state::<DevicesState>();
    let tracks = Tracks { library: &library, devices: &devices };
    let slot = playback.playing();

    let before = match cast.take(None) {
        Some((last, fallback)) => (true, status_of(&last, fallback)),
        None => (false, playback.player.status()),
    };
    let (was_casting, before) = before;
    cast.casting.store(opened.is_some(), Ordering::Relaxed);
    if let Some(session) = opened {
        *cast.session() = Some(session);
    }
    // The bookmark moves with the song, so the new output's resume finds it where it was left.
    playback.save_place(&library, &before);
    // Once the speaker is installed, so that the engine's stopping is dropped, not announced.
    if !was_casting && before.track_id.is_some() {
        playback.player.stop();
    }
    let _ = app.emit(TARGET_EVENT, device);

    if before.track_id.is_some()
        && let Some(playing) = slot.as_ref()
    {
        playback.start_at(&tracks, playing, before.position_ms, before.state == EngineState::Paused)?;
    }
    Ok(())
}

/// Opens the pane of System Settings where an AirPlay speaker is chosen as the Mac's output. The
/// engine plays to whatever the output is, and follows it when it changes.
#[tauri::command]
pub fn cast_open_airplay(state: State<'_, CastState>) -> CommandResult<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(SOUND_SETTINGS)
            .spawn()
            .map_err(|error| CommandError::from(format!("Couldn’t open Sound settings: {error}")))?;
        state.notice("Choose an AirPlay speaker under Output in Sound settings, or in Control Center.".to_owned());
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
        Err(CommandError::from("AirPlay is only available on a Mac.".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpStream};
    use std::sync::mpsc::{Receiver, channel};

    use unsub_library::MediaKind;

    use super::*;
    use crate::test_support::track;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/playback/tests/fixtures")
    }

    struct Rig {
        songs: Songs,
        server: Server,
        events: Receiver<PlayerEvent>,
        dir: tempfile::TempDir,
    }

    /// Songs served to a speaker of `kind`, from a real link server on loopback.
    fn rig(kind: Kind, art: Option<(&'static str, Vec<u8>)>) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let links: Arc<Links<Served>> = Arc::default();
        let server = Server::start_links(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), Arc::new(Open(Arc::clone(&links)))).unwrap();
        let (sender, events) = channel();
        let songs = Songs {
            kind,
            speaker: "Kitchen".into(),
            base: format!("http://{}", server.local_addr()),
            dir: dir.path().to_owned(),
            links,
            items: Mutex::default(),
            converted: Mutex::default(),
            art: Box::new(move |_: &ArtSource| art.clone().map(|(mime, bytes)| (mime.to_owned(), bytes))),
            events: sender,
            conversions: AtomicU64::new(0),
        };
        Rig { songs, server, events, dir }
    }

    fn song(file: &str, codec: &str, rate: u32, depth: u8) -> Item {
        let path = fixtures().join(file);
        let library_track = Track {
            sample_rate: Some(rate),
            bit_depth: Some(depth),
            title: "Tone".into(),
            artist: "Sine".into(),
            album: "Waves".into(),
            ..track(5, codec, MediaKind::Music)
        };
        Item::of_track(&library_track, &path, Some(ArtSource::Library(5)))
    }

    fn not_cancelled() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// A plain GET of `url`, headers and body.
    fn fetch(url: &str) -> (String, Vec<u8>) {
        let rest = url.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_once('/').unwrap();
        let mut socket = TcpStream::connect(host).unwrap();
        write!(socket, "GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
        let mut reply = Vec::new();
        socket.read_to_end(&mut reply).unwrap();
        let split = reply.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (String::from_utf8_lossy(&reply[..split]).into_owned(), reply[split + 4..].to_vec())
    }

    #[test]
    fn a_song_the_speaker_plays_is_served_as_it_is() {
        let rig = rig(Kind::Sonos, None);
        rig.songs.register(5, song("sine.flac", "FLAC", 44_100, 16));
        let media = rig.songs.media(5, &not_cancelled()).unwrap();

        assert!(media.url.ends_with(".flac"), "{}", media.url);
        assert_eq!(media.mime, "audio/flac");
        assert_eq!((media.title.as_str(), media.artist.as_str(), media.album.as_str()), ("Tone", "Sine", "Waves"));
        let (head, body) = fetch(&media.url);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, std::fs::read(fixtures().join("sine.flac")).unwrap());
        assert!(rig.events.try_recv().is_err(), "nothing was converted, so nothing to announce");
    }

    #[test]
    fn a_song_the_speaker_cannot_play_is_converted_to_flac_and_the_file_goes_with_its_link() {
        let rig = rig(Kind::Sonos, None);
        rig.songs.register(5, song("sine-alac.m4a", "ALAC", 44_100, 16));
        let media = rig.songs.media(5, &not_cancelled()).unwrap();

        assert!(media.url.ends_with(".flac") && media.mime == "audio/flac", "{media:?}");
        assert!(matches!(rig.events.try_recv(), Ok(PlayerEvent::Notice { message }) if message.contains("Converting “Tone” for Kitchen")));
        let (_, body) = fetch(&media.url);
        assert_eq!(&body[..4], b"fLaC");
        let files: Vec<_> = std::fs::read_dir(rig.dir.path()).unwrap().collect();
        assert_eq!(files.len(), 1);

        rig.songs.release(&media);
        assert_eq!(std::fs::read_dir(rig.dir.path()).unwrap().count(), 0);
        assert!(fetch(&media.url).0.starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn high_resolution_audio_is_brought_down_for_a_speaker_but_a_chromecast_plays_opus_as_it_is() {
        let sonos = rig(Kind::Sonos, None);
        sonos.songs.register(5, song("sine-24bit-96k.flac", "FLAC", 96_000, 24));
        let converted = sonos.songs.media(5, &not_cancelled()).unwrap();
        let (_, body) = fetch(&converted.url);
        assert_eq!(&body[..4], b"fLaC");
        let file = std::fs::read_dir(sonos.dir.path()).unwrap().next().unwrap().unwrap().path();
        let decoder = unsub_playback::Decoder::open(&file).unwrap();
        assert_eq!((decoder.sample_rate(), decoder.bit_depth()), (48_000, Some(24)), "96 kHz comes down to 48, the depth stays");

        let opus = song("sine.opus", "Opus", 48_000, 16);
        let sonos_opus = {
            sonos.songs.register(6, opus.clone());
            sonos.songs.media(6, &not_cancelled()).unwrap()
        };
        assert!(sonos_opus.url.ends_with(".flac"));
        let chromecast = rig(Kind::Chromecast, None);
        chromecast.songs.register(6, opus);
        let passed = chromecast.songs.media(6, &not_cancelled()).unwrap();
        assert!(passed.url.ends_with(".opus") && passed.mime == "audio/ogg", "{passed:?}");
        assert_eq!(std::fs::read_dir(chromecast.dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_song_nobody_registered_is_refused_rather_than_guessed_at() {
        let rig = rig(Kind::Sonos, None);
        assert!(rig.songs.media(404, &not_cancelled()).is_err());
    }

    #[test]
    fn cover_art_is_served_beside_the_song_and_goes_with_it() {
        let rig = rig(Kind::Upnp, Some(("image/png", b"png bytes".to_vec())));
        rig.songs.register(5, song("sine.flac", "FLAC", 44_100, 16));
        let media = rig.songs.media(5, &not_cancelled()).unwrap();
        let art = media.art_url.clone().unwrap();
        assert!(art.ends_with(".png"));
        let (head, body) = fetch(&art);
        assert!(head.to_lowercase().contains("content-type: image/png"), "{head}");
        assert_eq!(body, b"png bytes");

        rig.songs.release(&media);
        assert!(fetch(&art).0.starts_with("HTTP/1.1 404"));
        assert!(fetch(&media.url).0.starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn a_song_without_art_has_no_art_link() {
        let rig = rig(Kind::Upnp, None);
        rig.songs.register(5, song("sine.flac", "FLAC", 44_100, 16));
        assert_eq!(rig.songs.media(5, &not_cancelled()).unwrap().art_url, None);
    }

    #[test]
    fn every_song_gets_a_link_of_its_own_so_one_closing_does_not_close_another() {
        let rig = rig(Kind::Sonos, None);
        rig.songs.register(5, song("sine.flac", "FLAC", 44_100, 16));
        let (first, second) = (rig.songs.media(5, &not_cancelled()).unwrap(), rig.songs.media(5, &not_cancelled()).unwrap());
        assert_ne!(first.url, second.url);
        rig.songs.release(&first);
        assert!(fetch(&first.url).0.starts_with("HTTP/1.1 404"));
        assert!(fetch(&second.url).0.starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn only_the_secrets_it_issued_open_anything() {
        let rig = rig(Kind::Sonos, None);
        rig.songs.register(5, song("sine.flac", "FLAC", 44_100, 16));
        rig.songs.media(5, &not_cancelled()).unwrap();
        let base = &rig.songs.base;
        for path in ["link/0123456789abcdef0123456789abcdef.flac", "link/..%2fCargo.toml", "rest/ping", "", "Cargo.toml"] {
            assert!(fetch(&format!("{base}/{path}")).0.starts_with("HTTP/1.1 404"), "{path}");
        }
        drop(rig.server);
    }

    #[test]
    fn the_registry_remembers_only_the_latest_songs() {
        let rig = rig(Kind::Sonos, None);
        for id in 0..20 {
            rig.songs.register(id, Item::of_file(Path::new("/music/a.flac")));
        }
        assert!(rig.songs.item(0).is_none() && rig.songs.item(19).is_some());
        assert_eq!(lock(&rig.songs.items).len(), REMEMBERED_SONGS);
        rig.songs.register(19, Item::of_file(Path::new("/music/b.flac")));
        assert_eq!(lock(&rig.songs.items).len(), REMEMBERED_SONGS, "registering a song again replaces it");
    }

    #[test]
    fn a_song_is_judged_by_what_is_in_its_file_not_by_its_name_or_what_a_device_says() {
        let probed = |file: &str| probe(&fixtures().join(file)).unwrap();
        assert_eq!(probed("sine.mp3").codec, Codec::Mp3);
        assert_eq!(probed("sine.flac").codec, Codec::Flac);
        assert_eq!((probed("sine-24bit-96k.flac").sample_rate, probed("sine-24bit-96k.flac").bit_depth), (Some(96_000), Some(24)));
        assert_eq!(probed("sine-aac.m4a").codec, Codec::Aac);
        assert_eq!(probed("sine-alac.m4a").codec, Codec::Alac, "an m4a is AAC or ALAC, as the file says");
        assert_eq!(probed("sine.opus").codec, Codec::Opus);
        assert_eq!(probed("sine-22k.aiff").codec, Codec::Aiff);
        assert_eq!(probed("sine-mono.wav").codec, Codec::Wav);
        assert_eq!(probed("sine.wv").codec, Codec::Other);
        assert_eq!(probe(&fixtures().join("not-audio.txt")), None);
    }

    #[test]
    fn a_song_known_only_by_its_file_is_served_or_converted_as_the_file_requires() {
        let sonos = rig(Kind::Sonos, None);
        let served = |file: &str| {
            sonos.songs.register(9, Item::of_file(&fixtures().join(file)));
            let media = sonos.songs.media(9, &not_cancelled()).unwrap();
            sonos.songs.release(&media);
            media
        };
        assert!(served("sine.mp3").url.ends_with(".mp3"));
        assert!(served("sine-aac.m4a").url.ends_with(".m4a"));
        // A 24/96 FLAC from a shared library, an AIFF off a CD, an ALAC file of an iPod.
        for file in ["sine-24bit-96k.flac", "sine-22k.aiff", "sine-alac.m4a", "sine.wv"] {
            let media = served(file);
            assert!(media.url.ends_with(".flac") && media.mime == "audio/flac", "{file}: {media:?}");
        }
    }

    #[test]
    fn what_a_device_or_a_server_says_of_a_song_is_used_when_its_file_cannot_be_read() {
        let rig = rig(Kind::Sonos, None);
        let mut item = song("sine.flac", "FLAC", 44_100, 16);
        item.path = fixtures().join("does-not-exist.flac");
        rig.songs.register(5, item);
        let media = rig.songs.media(5, &not_cancelled()).unwrap();
        assert!(media.url.ends_with(".flac"), "served as it was said to be, for the speaker to find it missing");
    }

    #[test]
    fn a_shared_librarys_song_is_shown_by_its_own_details() {
        let song = Song {
            id: "s1".into(),
            title: "Shared Tune".into(),
            artist: Some("Neighbour".into()),
            album: Some("Over the Fence".into()),
            duration: Some(215),
            ..Song::default()
        };
        let item = Item::of_song(&song, Path::new("/cache/s1.flac"));
        assert_eq!(
            (item.title.as_str(), item.artist.as_str(), item.album.as_str(), item.duration_ms),
            ("Shared Tune", "Neighbour", "Over the Fence", 215_000)
        );
        let bare = Item::of_file(Path::new("/x/Some Song.flac"));
        assert_eq!((bare.title.as_str(), bare.artist.as_str(), bare.duration_ms), ("Some Song", "", 0));
    }

    #[test]
    fn each_session_converts_into_a_folder_of_its_own() {
        let cache = tempfile::tempdir().unwrap();
        let (first, second) = (session_dir(cache.path(), 0), session_dir(cache.path(), 1));
        assert_ne!(first, second);
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("7-0.flac"), b"fLaC").unwrap();

        // Ending the first, as switching to the second does, takes nothing of the second's.
        std::fs::remove_dir_all(&first).unwrap();
        assert!(second.join("7-0.flac").exists());
    }

    #[test]
    fn only_addresses_on_this_sessions_link_server_are_its_own() {
        let rig = rig(Kind::Sonos, None);
        let base = rig.songs.base.clone();
        assert!(rig.songs.serves(&format!("{base}/link/0a1b2c.flac")), "even a link long closed");
        assert!(!rig.songs.serves(&format!("{base}/other")));
        assert!(!rig.songs.serves("http://192.168.1.77:9000/link/0a1b2c.flac"), "a link on another machine");
        assert!(!rig.songs.serves("x-sonos-spotify:spotify%3atrack%3a1"));
        assert!(!rig.songs.serves(""));
    }

    #[test]
    fn secrets_are_read_from_links() {
        assert_eq!(secret_of("http://192.168.1.2:5000/link/0a1b2c.flac"), Some("0a1b2c"));
        assert_eq!(secret_of("http://192.168.1.2:5000/link/0a1b2c"), Some("0a1b2c"));
        assert_eq!(secret_of("http://192.168.1.2:5000/other"), None);
    }

    #[test]
    fn a_speakers_snapshot_reads_as_the_engines_status() {
        let snapshot = Snapshot { state: Speaker::Paused, id: Some(7), position_ms: 61_000, duration_ms: 180_000, volume: Some(0.4) };
        let status = status_of(&snapshot, 0.9);
        assert_eq!(
            (status.state, status.track_id, status.position_ms, status.duration_ms),
            (EngineState::Paused, Some(7), 61_000, 180_000)
        );
        assert_eq!(status.volume, 0.4);
        assert!(!status.bit_perfect && !status.eq_active && !status.live);
        let unknown = Snapshot { volume: None, state: Speaker::Buffering, ..snapshot };
        let status = status_of(&unknown, 0.9);
        assert_eq!((status.state, status.volume), (EngineState::Playing, 0.9));
    }

    #[test]
    fn what_the_speaker_does_reaches_the_app_as_the_events_the_engine_sends() {
        assert!(matches!(player_event(&Event::Started(3), "Kitchen", 1.0), Some(PlayerEvent::TrackStarted { track_id: 3 })));
        assert!(matches!(player_event(&Event::Ended(3), "Kitchen", 1.0), Some(PlayerEvent::TrackEnded { track_id: 3 })));
        assert!(matches!(
            player_event(&Event::Position { id: 3, position_ms: 10, duration_ms: 20 }, "Kitchen", 1.0),
            Some(PlayerEvent::Position { track_id: 3, position_ms: 10, duration_ms: 20 })
        ));
        let failed = player_event(&Event::Failed { id: Some(3), message: "no".into() }, "Kitchen", 1.0);
        assert!(matches!(failed, Some(PlayerEvent::Notice { message }) if message == "Kitchen couldn’t play the song: no"));
        assert!(player_event(&Event::Lost("gone".into()), "Kitchen", 1.0).is_none());
    }

    /// A speaker that remembers what it was given.
    struct Listener {
        loaded: Arc<Mutex<Vec<Media>>>,
        preloaded: Arc<Mutex<Vec<Media>>>,
    }

    impl unsub_cast::Receiver for Listener {
        fn load(&mut self, media: &Media, _start_ms: u64, _autoplay: bool) -> unsub_cast::Result<()> {
            lock(&self.loaded).push(media.clone());
            Ok(())
        }
        fn preload(&mut self, media: &Media) -> unsub_cast::Result<bool> {
            lock(&self.preloaded).push(media.clone());
            Ok(true)
        }
        fn play(&mut self) -> unsub_cast::Result<()> {
            Ok(())
        }
        fn pause(&mut self) -> unsub_cast::Result<()> {
            Ok(())
        }
        fn seek(&mut self, _position_ms: u64) -> unsub_cast::Result<()> {
            Ok(())
        }
        fn set_volume(&mut self, _volume: f32) -> unsub_cast::Result<()> {
            Ok(())
        }
        fn status(&mut self) -> unsub_cast::Result<unsub_cast::Status> {
            Err(CastError::Lost("not asked in this test".into()))
        }
        fn stop(&mut self) -> unsub_cast::Result<()> {
            Ok(())
        }
        fn disconnect(&mut self) {}
    }

    #[test]
    fn the_driver_hands_a_speaker_links_that_fetch_the_current_and_the_queued_song_and_nothing_else() {
        let rig = rig(Kind::Sonos, None);
        rig.songs.register(1, song("sine.flac", "FLAC", 44_100, 16));
        rig.songs.register(2, song("sine-alac.m4a", "ALAC", 44_100, 16));
        let (loaded, preloaded) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
        let songs = Arc::new(rig.songs);
        let (events, _heard) = channel();
        let mut driver = Driver::new(
            Box::new(Listener { loaded: Arc::clone(&loaded), preloaded: Arc::clone(&preloaded) }),
            songs.clone(),
            move |event| {
                let _ = events.send(event);
            },
        );
        // Songs are made ready on threads of their own, and taken in as they are.
        let until = |driver: &mut Driver, done: &dyn Fn() -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !done() {
                assert!(std::time::Instant::now() < deadline, "never ready");
                driver.apply_ready();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };

        driver.handle(Command::Play { id: 1, start_ms: 0, paused: false });
        until(&mut driver, &|| !lock(&loaded).is_empty());
        driver.handle(Command::Queue { id: 2 });
        until(&mut driver, &|| !lock(&preloaded).is_empty());

        let (now, next) = (lock(&loaded)[0].clone(), lock(&preloaded)[0].clone());
        assert_eq!(fetch(&now.url).1, std::fs::read(fixtures().join("sine.flac")).unwrap());
        assert_eq!(&fetch(&next.url).1[..4], b"fLaC", "the queued ALAC song is served converted");
        assert_eq!(songs.links.len(), 2, "one link each, and no others");

        driver.handle(Command::ClearQueue);
        assert!(fetch(&next.url).0.starts_with("HTTP/1.1 404"));
        driver.finish();
        assert!(fetch(&now.url).0.starts_with("HTTP/1.1 404"));
        assert!(songs.links.is_empty());
        drop(rig.server);
    }

    #[test]
    fn bytes_are_served_as_a_stream_with_their_length_and_files_by_path() {
        let links: Arc<Links<Served>> = Arc::default();
        let bytes = links.mint(Served::Bytes { bytes: Arc::from(&b"abc"[..]), mime: "image/png".into() }).unwrap();
        let file = links.mint(Served::File { path: "/music/a.flac".into(), mime: "audio/flac".into() }).unwrap();
        let open = Open(links);
        assert!(matches!(open.open(&bytes), Some(Stream::Reader { content_type, length: Some(3), .. }) if content_type == "image/png"));
        assert!(matches!(open.open(&file), Some(Stream::File { content_type, .. }) if content_type == "audio/flac"));
        assert!(open.open("nothing").is_none());
    }
}
