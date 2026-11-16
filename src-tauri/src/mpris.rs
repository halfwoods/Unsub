use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Str, Value};

const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
const BUS_NAME: &str = "org.mpris.MediaPlayer2.iTunes";

pub fn start(app: AppHandle) {
    thread::spawn(move || {
        if let Err(error) = serve(app) {
            eprintln!("[mpris] {error}");
        }
    });
}

fn serve(app: AppHandle) -> zbus::Result<()> {
    let connection = Connection::session()?;
    connection.request_name(BUS_NAME)?;
    connection.object_server().at(OBJECT_PATH, Root { app: app.clone() })?;
    connection.object_server().at(OBJECT_PATH, Player { app: app.clone() })?;
    let mut previous = String::new();
    let mut previous_track = None;
    loop {
        let playback = app.state::<crate::playback::PlaybackState>().status();
        let status = match playback.state {
            unsub_playback::PlaybackState::Playing => "Playing",
            unsub_playback::PlaybackState::Paused => "Paused",
            unsub_playback::PlaybackState::Stopped => "Stopped",
        };
        if status != previous {
            let changed = HashMap::from([("PlaybackStatus", OwnedValue::from(Str::from(status)))]);
            connection.emit_signal(
                None::<&str>, OBJECT_PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged",
                &("org.mpris.MediaPlayer2.Player", changed, Vec::<String>::new()),
            )?;
            previous = status.to_owned();
        }
        if playback.track_id != previous_track {
            let changed = HashMap::from([("Metadata", OwnedValue::from(metadata(&app)))]);
            connection.emit_signal(
                None::<&str>, OBJECT_PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged",
                &("org.mpris.MediaPlayer2.Player", changed, Vec::<String>::new()),
            )?;
            previous_track = playback.track_id;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn metadata(app: &AppHandle) -> HashMap<String, OwnedValue> {
    let playback = app.state::<crate::playback::PlaybackState>();
    let status = playback.status();
    let playing = playback.playing();
    if !matches!(playing.as_ref().map(|p| &p.source), Some(crate::playback::PlaySource::Library)) {
        return HashMap::new();
    }
    let Some(id) = status.track_id else { return HashMap::new() };
    let library = app.state::<crate::library::LibraryState>();
    let Ok(Some(track)) = library.queries().track(id) else { return HashMap::new() };
    let mut result = HashMap::new();
    result.insert("xesam:title".into(), OwnedValue::from(Str::from(track.title)));
    if let Ok(artist) = Value::new(vec![track.artist]).try_to_owned() {
        result.insert("xesam:artist".into(), artist);
    }
    result.insert("xesam:album".into(), OwnedValue::from(Str::from(track.album)));
    result
}

struct Root {
    app: AppHandle,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        if let Some(window) = self.app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool { false }
    #[zbus(property)]
    fn can_raise(&self) -> bool { true }
    #[zbus(property)]
    fn has_track_list(&self) -> bool { false }
    #[zbus(property)]
    fn identity(&self) -> &str { "iTunes" }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str { "itunes" }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> { Vec::new() }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> { Vec::new() }
}

struct Player {
    app: AppHandle,
}

impl Player {
    fn command(&self, command: &str) {
        let _ = self.app.emit("mpris://command", command.to_owned());
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) { self.command("next"); }
    fn previous(&self) { self.command("previous"); }
    fn play(&self) { self.command("play"); }
    fn pause(&self) { self.command("pause"); }
    fn play_pause(&self) { self.command("play-pause"); }

    #[zbus(property)]
    fn playback_status(&self) -> &'static str {
        use crate::playback::PlaybackState as State;
        match self.app.state::<State>().status().state {
            unsub_playback::PlaybackState::Playing => "Playing",
            unsub_playback::PlaybackState::Paused => "Paused",
            unsub_playback::PlaybackState::Stopped => "Stopped",
        }
    }
    #[zbus(property)]
    fn loop_status(&self) -> &'static str { "None" }
    #[zbus(property)]
    fn rate(&self) -> f64 { 1.0 }
    #[zbus(property)]
    fn shuffle(&self) -> bool { false }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> { metadata(&self.app) }
    #[zbus(property)]
    fn volume(&self) -> f64 { f64::from(self.app.state::<crate::playback::PlaybackState>().status().volume.min(1.0)) }
    #[zbus(property)]
    fn position(&self) -> i64 {
        self.app.state::<crate::playback::PlaybackState>().status().position_ms
            .saturating_mul(1_000).min(i64::MAX as u64) as i64
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 { 1.0 }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 { 1.0 }
    #[zbus(property)]
    fn can_go_next(&self) -> bool { true }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool { true }
    #[zbus(property)]
    fn can_play(&self) -> bool { true }
    #[zbus(property)]
    fn can_pause(&self) -> bool { true }
    #[zbus(property)]
    fn can_seek(&self) -> bool { false }
    #[zbus(property)]
    fn can_control(&self) -> bool { true }
}
