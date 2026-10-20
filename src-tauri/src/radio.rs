use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use unsub_library::{NewStation, Station as SavedStation};
use unsub_playback::{LiveFormat, PlayerEvent, TrackOptions};
use unsub_radio::{CancelHandle, Directory, Event, Format, GENRES, NowPlaying, Station as DirectoryStation, StationInfo, Tag, genres_of};

use crate::error::{CommandError, CommandResult};
use crate::library::{self, LibraryState};
use crate::playback::{PLAYER_EVENT, PlaybackState, Playing, now_unix};

/// Carries `{ stationId, artist, title }`; both names are null when the station clears its title.
pub const NOW_PLAYING_EVENT: &str = "radio://now-playing";

/// Carries the station being tuned in, whole; sent again when its details change, and `null` when
/// it is removed from the library.
pub const STATION_EVENT: &str = "radio://station";

/// Carries `"connecting"` while a station is being reached and `"reconnecting"` after a drop, and
/// `null` otherwise.
pub const LINK_EVENT: &str = "radio://link";

/// The engine starts a live stream once about two seconds of it are decoded, so a title shown the
/// moment it arrives would lead what is heard by that long.
const PRE_ROLL: Duration = Duration::from_secs(2);

fn to_error(error: unsub_radio::Error) -> CommandError {
    CommandError::from(error.to_string())
}

/// What a station says is playing, as the LCD shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
struct Title {
    artist: Option<String>,
    title: Option<String>,
}

impl Title {
    fn of(track: &NowPlaying) -> Self {
        if let Some(title) = &track.title {
            return Self { artist: track.artist.clone(), title: Some(title.clone()) };
        }
        // Text the station didn't split is shown whole, as the title.
        let raw = track.raw.trim();
        Self { artist: None, title: (!raw.is_empty()).then(|| raw.to_owned()) }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum Link {
    Connecting,
    Reconnecting,
}

fn set_link(app: &AppHandle, link: Option<Link>) {
    let _ = app.emit(LINK_EVENT, link);
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NowPlayingEvent {
    station_id: i64,
    #[serde(flatten)]
    title: Title,
}

/// Names one start of a station, so work it leaves behind (a slow connection, a title waiting out
/// the pre-roll) can tell that it has been replaced.
#[derive(Clone, Copy)]
struct Ticket(u64);

/// The station being listened to, or the last one, which Play opens again. Songs and stations
/// never play together: starting one ends the other (see `take_over` and `RadioState::end_live`).
#[derive(Default)]
struct Session {
    starts: u64,
    station: Option<i64>,
    name: String,
    /// Whether the connection is still being made, which can take many seconds. The stream is only
    /// attached once it is open.
    opening: bool,
    /// Wakes a read stuck on a dead connection. Dropping the stream doesn't until that read returns.
    cancel: Option<CancelHandle>,
    title: Title,
    /// A failure reaches us twice, as the stream's `Failed` and as the engine's `LiveEnded`; the
    /// listener is told once.
    told: bool,
}

impl Session {
    fn begin(&mut self, station: i64, name: &str) -> Ticket {
        self.cancel();
        *self = Session { starts: self.starts + 1, station: Some(station), name: name.to_owned(), opening: true, ..Session::default() };
        Ticket(self.starts)
    }

    fn end(&mut self) {
        self.cancel();
        self.starts += 1;
        self.station = None;
        self.opening = false;
    }

    /// Ends the session if it is `station`'s, which has left the library.
    fn forget(&mut self, station: i64) -> bool {
        let held = self.station == Some(station);
        if held {
            self.end();
        }
        held
    }

    /// Gives up on a connection being made, keeping the station for a later Play. Whether there was one.
    fn abandon(&mut self) -> bool {
        let was = self.opening;
        if was {
            self.starts += 1;
            self.opening = false;
        }
        was
    }

    /// Whether there was a stream to cancel.
    fn cancel(&mut self) -> bool {
        self.cancel.take().map(|cancel| cancel.cancel()).is_some()
    }

    fn current(&self, ticket: Ticket) -> bool {
        self.station.is_some() && ticket.0 == self.starts
    }

    /// Whether a stream is connected for the current start, as opposed to one stopped on purpose
    /// or not yet opened, whose ending is nobody's news.
    fn streaming(&self) -> bool {
        self.cancel.is_some()
    }

    fn listening(&self, ticket: Ticket) -> bool {
        self.current(ticket) && self.streaming()
    }

    /// Whether `ticket` is still the current start; if so, `cancel` is the stream to cancel with it.
    fn attach(&mut self, ticket: Ticket, cancel: CancelHandle) -> bool {
        let current = self.current(ticket);
        if current {
            self.cancel = Some(cancel);
            self.opening = false;
        }
        current
    }

    /// Whether the connection failing to open is news: not if the start was replaced or abandoned.
    fn open_failed(&mut self, ticket: Ticket) -> bool {
        let tell = self.current(ticket) && self.opening;
        self.opening &= !tell;
        tell
    }

    /// Whether `name` is the station's name now, which it isn't if another start has replaced it.
    fn rename(&mut self, ticket: Ticket, name: &str) -> bool {
        let current = self.current(ticket);
        if current {
            name.clone_into(&mut self.name);
        }
        current
    }

    /// Whether `title` is news, which it isn't when stations repeat it after reconnecting.
    fn change_title(&mut self, ticket: Ticket, title: &Title) -> bool {
        let changed = self.current(ticket) && self.title != *title;
        if changed {
            self.title.clone_from(title);
        }
        changed
    }

    /// The name of the station to tell the listener has stopped, if the stream's own report of
    /// failing is news.
    fn fail(&mut self, ticket: Ticket) -> Option<String> {
        let tell = self.listening(ticket) && !self.told;
        self.told |= tell;
        tell.then(|| self.name.clone())
    }

    /// The name of the station to tell the listener has stopped, if the engine's report of it is news.
    fn ended(&mut self, station: i64) -> Option<String> {
        let tell = self.station == Some(station) && self.streaming() && !self.told;
        self.told |= tell;
        tell.then(|| self.name.clone())
    }
}

/// A station starting takes the place of the songs queued before it, and of a station before it.
fn take_over(queue: &mut Option<Playing>, session: &mut Session, station: &SavedStation) -> Ticket {
    *queue = None;
    session.begin(station.id, &station.name)
}

/// The station directory, and the one live stream that may be playing.
pub struct RadioState {
    directory: Directory,
    /// Taken after `PlaybackState::playing`, like the rest of its locks.
    session: Mutex<Session>,
}

impl RadioState {
    pub fn new(directory_cache: PathBuf) -> Self {
        Self { directory: Directory::new().with_cache_file(directory_cache), session: Mutex::default() }
    }

    fn session(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A song is starting: the station stops, and Play no longer reopens it.
    pub(crate) fn end_live(&self) {
        self.session().end();
    }
}

fn live_format(format: Format) -> LiveFormat {
    match format {
        Format::Mp3 => LiveFormat::Mp3,
        Format::AacAdts => LiveFormat::AacAdts,
        Format::Ogg => LiveFormat::Ogg,
        Format::Opus => LiveFormat::Opus,
        Format::Flac => LiveFormat::Flac,
        Format::MpegTs => LiveFormat::MpegTs,
        Format::Fmp4 => LiveFormat::Fmp4,
    }
}

fn non_empty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// How a directory station is kept in the library. Its genre is ours, the one it is listed under.
fn new_station(station: &DirectoryStation) -> NewStation {
    let genre = genres_of(station).first().map(|genre| genre.name.to_owned()).or_else(|| station.tags.first().cloned()).unwrap_or_default();
    NewStation {
        name: station.name.clone(),
        url: station.url.clone(),
        genre,
        bitrate_kbps: (station.bitrate_kbps > 0).then_some(station.bitrate_kbps),
        codec: station.codec.clone(),
        homepage: non_empty(&station.homepage),
        favicon_url: non_empty(&station.favicon),
        radio_browser_uuid: non_empty(&station.uuid),
    }
}

#[derive(Serialize)]
pub struct GenreCount {
    name: &'static str,
    count: u32,
}

/// How many streams each genre has: Radio Browser counts by tag, so a genre is its tags' sum.
fn genre_counts(tags: &[Tag]) -> Vec<GenreCount> {
    GENRES
        .iter()
        .map(|genre| GenreCount {
            name: genre.name,
            count: tags.iter().filter(|tag| genre.tags.contains(&tag.name.to_lowercase().as_str())).map(|tag| tag.station_count).sum(),
        })
        .collect()
}

fn notice(app: &AppHandle, message: String) {
    let _ = app.emit(PLAYER_EVENT, PlayerEvent::Notice { message });
}

fn stopped_message(name: &str, reason: &str) -> String {
    format!("“{name}” stopped: {reason}")
}

/// Starts `station`, replacing whatever is playing; the connection is made in the background.
fn start(app: &AppHandle, station: SavedStation) {
    let radio = app.state::<RadioState>();
    let playback = app.state::<PlaybackState>();
    let ticket = {
        let mut queue = playback.playing();
        playback.stop_for_station();
        take_over(&mut queue, &mut radio.session(), &station)
    };
    let _ = app.emit(STATION_EVENT, &station);
    set_link(app, Some(Link::Connecting));
    let app = app.clone();
    std::thread::spawn(move || connect(&app, ticket, &station));
}

fn connect(app: &AppHandle, ticket: Ticket, station: &SavedStation) {
    let radio = app.state::<RadioState>();
    let mut stream = match unsub_radio::open(&station.url) {
        Ok(stream) => stream,
        Err(error) => {
            if radio.session().open_failed(ticket) {
                notice(app, format!("Couldn’t play “{}”: {error}", station.name));
                set_link(app, None);
            }
            return;
        }
    };
    let events = stream.take_events();
    let info = stream.info().clone();
    let format = live_format(stream.format());
    let cancel = stream.cancel_handle();
    {
        let mut session = radio.session();
        // Dropping the stream here cancels it.
        if !session.attach(ticket, cancel) {
            return;
        }
        app.state::<PlaybackState>().player.play_live(station.id, Box::new(stream), format, TrackOptions::default());
    }
    set_link(app, None);
    played(app, station);
    if let Some(named) = named_by_stream(station, &info) {
        rename(app, ticket, station.id, &named);
    }
    if let Some(events) = events {
        forward(app, ticket, station, &events);
    }
}

/// Counts the start as a play, and as a listen for the directory.
fn played(app: &AppHandle, station: &SavedStation) {
    let recorded = app.state::<LibraryState>().queries().record_play(station.id, now_unix());
    match recorded {
        Ok(()) => {
            let _ = app.emit(library::CHANGED_EVENT, ());
        }
        Err(error) => eprintln!("[radio] {error:?}"),
    }
    if let Some(uuid) = station.radio_browser_uuid.clone() {
        let app = app.clone();
        // A request to the directory can take seconds, and titles are waiting.
        std::thread::spawn(move || {
            if let Err(error) = app.state::<RadioState>().directory.count_click(&uuid) {
                eprintln!("[radio] {error}");
            }
        });
    }
}

/// Passes the stream's titles on to the UI as they are heard, and its failure on as a notice.
/// Returns when the stream is dropped.
fn forward(app: &AppHandle, ticket: Ticket, station: &SavedStation, events: &Receiver<Event>) {
    let radio = app.state::<RadioState>();
    for event in events {
        match event {
            Event::NowPlaying { track, .. } => {
                std::thread::sleep(PRE_ROLL);
                let title = Title::of(&track);
                if radio.session().change_title(ticket, &title) {
                    let _ = app.emit(NOW_PLAYING_EVENT, NowPlayingEvent { station_id: station.id, title });
                }
            }
            Event::Failed(reason) => {
                let name = radio.session().fail(ticket);
                if let Some(name) = name {
                    notice(app, stopped_message(&name, &reason));
                    set_link(app, None);
                }
            }
            Event::Reconnecting { .. } => {
                let listening = radio.session().listening(ticket);
                if listening {
                    set_link(app, Some(Link::Reconnecting));
                }
            }
            Event::Reconnected => {
                let listening = radio.session().listening(ticket);
                if listening {
                    set_link(app, None);
                }
            }
            Event::Skipped { .. } => {}
        }
    }
}

/// The engine reports a station over once what it had decoded has played out.
pub(crate) fn live_ended(app: &AppHandle, station: i64, reason: &str) {
    let name = app.state::<RadioState>().session().ended(station);
    if let Some(name) = name {
        notice(app, stopped_message(&name, reason));
        set_link(app, None);
    }
}

/// The engine stopped a live stream because the listener paused it. This also wakes a read stuck
/// on a dead connection.
pub(crate) fn paused(app: &AppHandle) {
    let cancelled = app.state::<RadioState>().session().cancel();
    if cancelled {
        set_link(app, None);
    }
}

/// Play/Pause with the engine stopped: a station still connecting is given up on, and one that was
/// paused or dropped is tuned in again. False when there is no station for the press, which then
/// belongs to the song queue.
pub(crate) fn toggle_stopped(app: &AppHandle) -> bool {
    let radio = app.state::<RadioState>();
    let (abandoned, station) = {
        let mut session = radio.session();
        (session.abandon(), session.station)
    };
    if abandoned {
        set_link(app, None);
        return true;
    }
    let Some(station) = station else { return false };
    if play(app, station).is_ok() {
        return true;
    }
    // It has left the library, so there is nothing to tune in again.
    radio.end_live();
    false
}

/// What the stream calls itself, for a station kept under only its address.
fn named_by_stream(station: &SavedStation, info: &StationInfo) -> Option<NewStation> {
    let name = info.name.as_ref().filter(|_| station.url.starts_with(&station.name))?;
    Some(NewStation {
        name: name.clone(),
        url: station.url.clone(),
        genre: info.genre.clone().unwrap_or_else(|| station.genre.clone()),
        bitrate_kbps: info.bitrate_kbps.or(station.bitrate_kbps),
        codec: station.codec.clone(),
        homepage: station.homepage.clone(),
        favicon_url: station.favicon_url.clone(),
        radio_browser_uuid: station.radio_browser_uuid.clone(),
    })
}

fn rename(app: &AppHandle, ticket: Ticket, id: i64, named: &NewStation) {
    let state = app.state::<LibraryState>();
    let renamed = {
        let mut library = state.queries();
        library.update_station(id, named).and_then(|()| library.station(id))
    };
    match renamed {
        Ok(Some(station)) => {
            let current = app.state::<RadioState>().session().rename(ticket, &station.name);
            if current {
                let _ = app.emit(STATION_EVENT, &station);
            }
            let _ = app.emit(library::CHANGED_EVENT, ());
        }
        Ok(None) => {}
        Err(error) => eprintln!("[radio] {error:?}"),
    }
}

/// Plays a station kept in the library, which is also how Play reopens one that was paused.
pub(crate) fn play(app: &AppHandle, id: i64) -> CommandResult<()> {
    let station = app
        .state::<LibraryState>()
        .queries()
        .station(id)?
        .ok_or_else(|| CommandError::from("that station is no longer in the library".to_owned()))?;
    start(app, station);
    Ok(())
}

fn add(app: &AppHandle, station: &NewStation) -> CommandResult<i64> {
    let state = app.state::<LibraryState>();
    let mut library = state.queries();
    let before = library.stations()?.len();
    let id = library.add_station(station)?;
    // A station already kept is left as it was, so the lists have nothing new to show.
    let created = library.stations()?.len() > before;
    drop(library);
    if created {
        let _ = app.emit(library::CHANGED_EVENT, ());
    }
    Ok(id)
}

#[tauri::command]
pub async fn radio_genres(radio: State<'_, RadioState>) -> CommandResult<Vec<GenreCount>> {
    Ok(genre_counts(&radio.directory.tags().map_err(to_error)?))
}

#[tauri::command]
pub async fn radio_stations(radio: State<'_, RadioState>, genre: String, offset: u32, limit: u32) -> CommandResult<Vec<DirectoryStation>> {
    let found = unsub_radio::genre(&genre).ok_or_else(|| CommandError::from(format!("there is no genre called {genre}")))?;
    radio.directory.stations_by_genre(found, offset, limit).map_err(to_error)
}

#[tauri::command]
pub async fn radio_search(radio: State<'_, RadioState>, query: String, limit: u32) -> CommandResult<Vec<DirectoryStation>> {
    radio.directory.search(&query, limit).map_err(to_error)
}

#[tauri::command]
pub fn radio_my_stations(library: State<'_, LibraryState>) -> CommandResult<Vec<SavedStation>> {
    Ok(library.queries().stations()?)
}

#[tauri::command]
pub fn radio_recent(library: State<'_, LibraryState>, limit: usize) -> CommandResult<Vec<SavedStation>> {
    Ok(library.queries().recently_played_stations(limit)?)
}

#[tauri::command]
pub fn radio_add(app: AppHandle, station: DirectoryStation) -> CommandResult<i64> {
    add(&app, &new_station(&station))
}

#[tauri::command]
pub fn radio_remove(app: AppHandle, library: State<'_, LibraryState>, id: i64) -> CommandResult<()> {
    library.queries().remove_station(id)?;
    // A station removed while it plays, connects or waits to be tuned in again is gone for good.
    if app.state::<RadioState>().session().forget(id) {
        app.state::<PlaybackState>().player.stop();
        set_link(&app, None);
        let _ = app.emit(STATION_EVENT, None::<SavedStation>);
    }
    let _ = app.emit(library::CHANGED_EVENT, ());
    Ok(())
}

#[tauri::command]
pub fn radio_play(app: AppHandle, station_id: i64) -> CommandResult<()> {
    play(&app, station_id)
}

/// A station from the directory is kept in My Stations as soon as it is played.
#[tauri::command]
pub fn radio_play_directory(app: AppHandle, station: DirectoryStation) -> CommandResult<()> {
    let id = add(&app, &new_station(&station))?;
    play(&app, id)
}

/// File › Open Stream…: any stream address, or a `.pls` or `.m3u` naming one.
#[tauri::command]
pub fn radio_open_stream(app: AppHandle, url: String) -> CommandResult<()> {
    let id = add(&app, &NewStation { url: url.trim().to_owned(), ..NewStation::default() })?;
    play(&app, id)
}

/// Adds the stations a `.pls` or `.m3u` lists; returns their ids.
#[tauri::command]
pub fn radio_import(app: AppHandle, library: State<'_, LibraryState>, path: PathBuf) -> CommandResult<Vec<i64>> {
    let ids = library.queries().import_playlist_file(&path)?;
    if !ids.is_empty() {
        let _ = app.emit(library::CHANGED_EVENT, ());
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::PlaySource;
    use crate::queue::{PlayQueue, RepeatMode};

    fn saved(id: i64, name: &str) -> SavedStation {
        SavedStation {
            id,
            name: name.into(),
            url: format!("http://radio.example/{id}"),
            genre: String::new(),
            bitrate_kbps: None,
            codec: String::new(),
            homepage: None,
            favicon_url: None,
            radio_browser_uuid: None,
            play_count: 0,
            last_played: None,
        }
    }

    fn directory_station() -> DirectoryStation {
        DirectoryStation {
            uuid: "960e57c5-0601-11e8-ae97-52543be04c81".into(),
            name: "SomaFM Groove Salad".into(),
            url: "https://ice1.somafm.com/groovesalad-128-mp3".into(),
            homepage: "https://somafm.com/groovesalad/".into(),
            favicon: "https://somafm.com/img/groovesalad120.png".into(),
            tags: vec!["chillout".into(), "ambient".into(), "downtempo".into()],
            country: "United States".into(),
            codec: "MP3".into(),
            bitrate_kbps: 128,
            is_hls: false,
            votes: 4000,
        }
    }

    fn queued() -> Option<Playing> {
        Some(Playing {
            source: PlaySource::Library,
            queue: PlayQueue::new(vec![1, 2, 3], 0, false, RepeatMode::Off).unwrap(),
            party: false,
        })
    }

    fn playing_now(session: &mut Session, id: i64) -> (Ticket, CancelHandle) {
        let ticket = session.begin(id, "Station");
        let cancel = CancelHandle::new();
        assert!(session.attach(ticket, cancel.clone()));
        (ticket, cancel)
    }

    #[test]
    fn every_stream_format_maps_to_the_engines_hint() {
        let formats = [
            (Format::Mp3, LiveFormat::Mp3),
            (Format::AacAdts, LiveFormat::AacAdts),
            (Format::Ogg, LiveFormat::Ogg),
            (Format::Opus, LiveFormat::Opus),
            (Format::Flac, LiveFormat::Flac),
            (Format::MpegTs, LiveFormat::MpegTs),
            (Format::Fmp4, LiveFormat::Fmp4),
        ];
        for (format, expected) in formats {
            assert_eq!(live_format(format), expected, "{format:?}");
        }
    }

    #[test]
    fn a_directory_station_is_kept_under_our_genre_with_its_directory_id() {
        let kept = new_station(&directory_station());
        assert_eq!(
            kept,
            NewStation {
                name: "SomaFM Groove Salad".into(),
                url: "https://ice1.somafm.com/groovesalad-128-mp3".into(),
                genre: "Ambient".into(),
                bitrate_kbps: Some(128),
                codec: "MP3".into(),
                homepage: Some("https://somafm.com/groovesalad/".into()),
                favicon_url: Some("https://somafm.com/img/groovesalad120.png".into()),
                radio_browser_uuid: Some("960e57c5-0601-11e8-ae97-52543be04c81".into()),
            }
        );
    }

    #[test]
    fn what_the_directory_leaves_out_stays_out_of_the_library() {
        let bare = DirectoryStation {
            homepage: String::new(),
            favicon: String::new(),
            tags: vec!["polka".into()],
            bitrate_kbps: 0,
            uuid: String::new(),
            ..directory_station()
        };
        let kept = new_station(&bare);
        assert_eq!((kept.homepage, kept.favicon_url, kept.bitrate_kbps, kept.radio_browser_uuid), (None, None, None, None));
        assert_eq!(kept.genre, "polka");
        assert_eq!(new_station(&DirectoryStation { tags: Vec::new(), ..bare }).genre, "");
    }

    #[test]
    fn genres_count_the_streams_of_their_tags() {
        let tag = |name: &str, station_count| Tag { name: name.into(), station_count };
        let counts = genre_counts(&[tag("ambient", 100), tag("Chillout", 52), tag("polka", 9)]);
        let count = |name: &str| counts.iter().find(|genre| genre.name == name).map(|genre| genre.count);
        assert_eq!(count("Ambient"), Some(152));
        assert_eq!(count("Jazz"), Some(0));
        assert_eq!(counts.len(), GENRES.len());
    }

    fn title(artist: Option<&str>, title: Option<&str>) -> Title {
        Title { artist: artist.map(str::to_owned), title: title.map(str::to_owned) }
    }

    #[test]
    fn the_title_has_artist_and_title_when_the_station_gave_both() {
        let track =
            NowPlaying { raw: "Boards of Canada - Roygbiv".into(), artist: Some("Boards of Canada".into()), title: Some("Roygbiv".into()) };
        assert_eq!(Title::of(&track), title(Some("Boards of Canada"), Some("Roygbiv")));
    }

    #[test]
    fn the_title_is_the_raw_text_when_it_does_not_split() {
        let named = NowPlaying { raw: "Morning Show".into(), artist: None, title: Some("Morning Show".into()) };
        assert_eq!(Title::of(&named), title(None, Some("Morning Show")));
        let unparsed = NowPlaying { raw: "  Station Jingle ".into(), artist: Some("Stray".into()), title: None };
        assert_eq!(Title::of(&unparsed), title(None, Some("Station Jingle")));
    }

    #[test]
    fn a_cleared_title_is_empty() {
        let cleared = NowPlaying { raw: "  ".into(), artist: None, title: None };
        assert_eq!(Title::of(&cleared), Title::default());
    }

    #[test]
    fn a_station_taking_over_drops_the_song_queue_and_the_station_before_it() {
        let mut queue = queued();
        let mut session = Session::default();
        let (first, first_stream) = playing_now(&mut session, 7);

        let second = take_over(&mut queue, &mut session, &saved(8, "Next"));

        assert!(queue.is_none());
        assert!(first_stream.is_cancelled());
        assert!(!session.current(first) && session.current(second));
        assert_eq!(session.station, Some(8));
        assert!(!session.attach(first, CancelHandle::new()), "the earlier connection must not play");
    }

    #[test]
    fn a_song_starting_stops_the_station_and_forgets_it() {
        let mut session = Session::default();
        let (ticket, stream) = playing_now(&mut session, 7);

        session.end();

        assert!(stream.is_cancelled());
        assert_eq!(session.station, None);
        assert!(!session.current(ticket));
        assert!(!session.attach(ticket, CancelHandle::new()), "a connection still opening must not play over the song");
    }

    #[test]
    fn a_paused_station_is_remembered_so_play_can_reopen_it() {
        let mut session = Session::default();
        let (_, stream) = playing_now(&mut session, 7);

        session.cancel();

        assert!(stream.is_cancelled());
        assert_eq!(session.station, Some(7));
        assert_eq!(session.ended(7), None, "stopping on purpose is not a failure");
    }

    #[test]
    fn a_failure_is_told_once_whichever_report_comes_first() {
        let mut session = Session::default();
        let (ticket, _) = playing_now(&mut session, 7);
        assert_eq!(session.fail(ticket), Some("Station".to_owned()));
        assert_eq!(session.ended(7), None);

        let (ticket, _) = playing_now(&mut session, 7);
        assert_eq!(session.ended(7), Some("Station".to_owned()));
        assert_eq!(session.fail(ticket), None);
    }

    #[test]
    fn nothing_is_told_about_a_station_that_was_replaced_or_never_connected() {
        let mut session = Session::default();
        let (old, _) = playing_now(&mut session, 7);
        let ticket = session.begin(7, "Station");
        assert_eq!(session.fail(old), None);
        assert_eq!(session.ended(7), None, "the old stream's ending must not be taken for the new one's");
        assert_eq!(session.fail(ticket), None, "nothing is connected yet");
        assert!(!session.listening(ticket));
    }

    #[test]
    fn a_title_is_news_once_and_only_for_the_current_start() {
        let mut session = Session::default();
        let (ticket, _) = playing_now(&mut session, 7);
        let song = title(Some("A"), Some("B"));
        assert!(session.change_title(ticket, &song));
        assert!(!session.change_title(ticket, &song), "a repeat after reconnecting");
        assert!(session.change_title(ticket, &Title::default()));

        let newer = session.begin(8, "Other");
        assert!(!session.change_title(ticket, &song));
        assert!(session.change_title(newer, &song), "a new start begins without a title");
    }

    #[test]
    fn a_start_is_opening_until_its_stream_is_attached() {
        let mut session = Session::default();
        let ticket = session.begin(7, "Station");
        assert!(session.opening);
        assert!(session.attach(ticket, CancelHandle::new()));
        assert!(!session.opening);
        assert!(!session.abandon(), "a live stream is paused, not abandoned");
    }

    #[test]
    fn abandoning_a_connection_keeps_the_station_and_turns_the_late_stream_away() {
        let mut session = Session::default();
        let ticket = session.begin(7, "Station");

        assert!(session.abandon());

        assert_eq!(session.station, Some(7), "a later Play connects again");
        assert!(!session.opening);
        assert!(!session.attach(ticket, CancelHandle::new()), "the connection that finishes late must not play");
        assert!(!session.abandon(), "there is nothing left to abandon");
    }

    #[test]
    fn a_failure_to_open_is_told_once_and_only_while_the_start_is_current() {
        let mut session = Session::default();
        let ticket = session.begin(7, "Station");
        assert!(session.open_failed(ticket));
        assert!(!session.open_failed(ticket));

        let replaced = session.begin(7, "Station");
        session.begin(8, "Other");
        assert!(!session.open_failed(replaced));

        let abandoned = session.begin(9, "Third");
        session.abandon();
        assert!(!session.open_failed(abandoned));

        let (live, _) = playing_now(&mut session, 7);
        assert!(!session.open_failed(live), "an open stream did not fail to open");
    }

    #[test]
    fn a_station_that_leaves_the_library_is_forgotten_wherever_it_is_in_its_life() {
        let mut session = Session::default();
        let (_, stream) = playing_now(&mut session, 7);
        assert!(!session.forget(8), "another station's removal changes nothing");
        assert_eq!(session.station, Some(7));

        assert!(session.forget(7));
        assert!(stream.is_cancelled());
        assert_eq!(session.station, None, "Play has no station to reopen");

        let ticket = session.begin(7, "Station");
        assert!(session.forget(7));
        assert!(!session.attach(ticket, CancelHandle::new()), "a connection still opening must not play a removed station");
    }

    #[test]
    fn a_rename_only_reaches_the_start_it_was_made_for() {
        let mut session = Session::default();
        let (old, _) = playing_now(&mut session, 7);
        let (current, _) = playing_now(&mut session, 7);

        assert!(!session.rename(old, "Stale"));
        assert!(session.rename(current, "Groove Salad"));
        assert_eq!(session.ended(7), Some("Groove Salad".to_owned()), "later notices use the new name");
    }

    fn by_address() -> SavedStation {
        let station = saved(5, "");
        SavedStation { name: station.url.clone(), ..station }
    }

    fn icy(name: Option<&str>, genre: Option<&str>, bitrate_kbps: Option<u32>) -> StationInfo {
        StationInfo { name: name.map(str::to_owned), genre: genre.map(str::to_owned), bitrate_kbps, ..StationInfo::default() }
    }

    #[test]
    fn a_station_kept_by_its_address_takes_the_name_its_stream_gives() {
        let station = SavedStation { genre: "Jazz".into(), codec: "MP3".into(), ..by_address() };

        let named = named_by_stream(&station, &icy(Some("Groove Salad"), Some("Ambient"), Some(128))).unwrap();
        assert_eq!((named.name.as_str(), named.genre.as_str(), named.bitrate_kbps), ("Groove Salad", "Ambient", Some(128)));
        assert_eq!((named.url, named.codec), (station.url.clone(), "MP3".to_owned()));

        let bare = named_by_stream(&station, &icy(Some("Groove Salad"), None, None)).unwrap();
        assert_eq!((bare.genre.as_str(), bare.bitrate_kbps), ("Jazz", None), "what the stream leaves out is kept");
    }

    #[test]
    fn a_stream_does_not_rename_a_station_that_has_a_name_or_gives_none() {
        assert_eq!(named_by_stream(&saved(5, "My Jazz"), &icy(Some("Groove Salad"), None, None)), None);
        assert_eq!(named_by_stream(&by_address(), &icy(None, Some("Ambient"), Some(128))), None);
    }

    #[test]
    fn a_long_address_still_counts_as_the_stations_name() {
        let url = format!("http://radio.example/{}", "a".repeat(300));
        let station = SavedStation { url: url.clone(), name: url.chars().take(200).collect(), ..saved(5, "") };
        assert!(named_by_stream(&station, &icy(Some("Groove Salad"), None, None)).is_some());
    }
}
