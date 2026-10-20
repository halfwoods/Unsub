//! Live streams.
//!
//! [`open`] gives back a [`LiveStream`] that reads as one endless run of audio bytes whatever the
//! station does underneath: ICY metadata is stripped and reported as [`Event`]s, a dropped
//! connection is re-established, and an HLS playlist is polled and its segments joined.
//!
//! # Reconnecting
//!
//! When a connection ends or goes silent for longer than the stall timeout, `read` does not fail:
//! it reconnects with a growing delay (half a second, doubling, capped) and carries on from the new
//! connection's first byte. Nothing is spliced, so the decoder sees a gap of whatever the server
//! missed, usually a second or two, and may need to resynchronise to the next frame. After
//! [`Options::max_failures`] attempts in a row without receiving audio, `read` returns the last
//! error and an [`Event::Failed`] is sent. [`Event::Reconnecting`] and [`Event::Reconnected`] let
//! the interface say "Reconnecting…" meanwhile.
//!
//! A response that announces a `Content-Length` and carries no ICY headers is a file, not a
//! station, and is read to its end without reconnecting.

pub(crate) mod format;
mod hls;
mod icy;

use std::{
    io::{self, Read},
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use url::Url;

pub use format::Format;
pub use icy::NowPlaying;

use crate::{
    error::{Error, Result, is_cancelled},
    http::{CancelHandle, Headers, Http, HttpConfig, Response, parse_url},
    playlist::{Playlist, Probed, probe},
};

use icy::Demuxer;

/// How many playlists-within-playlists are followed before giving up.
const MAX_PLAYLIST_DEPTH: u32 = 3;
/// How much of the start of a stream is read to tell what it is.
const SNIFF_LIMIT: usize = 16 * 1024;
const SNIFF_STEP: usize = 2048;

/// What the station says about itself, from the `icy-*` response headers. Everything is optional
/// because most servers send some of it and many send none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationInfo {
    pub name: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub genre: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
}

impl StationInfo {
    fn from_headers(headers: &Headers) -> Self {
        let text = |name| {
            headers
                .get(name)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        Self {
            name: text("icy-name"),
            // Some servers send "128,128" or "128 kbps".
            bitrate_kbps: headers.get("icy-br").and_then(|value| {
                value
                    .trim()
                    .split(|c: char| !c.is_ascii_digit())
                    .find(|part| !part.is_empty())?
                    .parse()
                    .ok()
            }),
            genre: text("icy-genre"),
            description: text("icy-description"),
            url: text("icy-url"),
        }
    }
}

/// Things that happen to a stream besides audio arriving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The station announced a new title. `offset` is the number of audio bytes, counted from the
    /// start of the stream, that come before the audio this title describes: the player can hold
    /// the title back until it has played that many.
    NowPlaying {
        track: NowPlaying,
        offset: u64,
    },
    /// The connection broke; this is the `attempt`th try (from 1) at restoring it.
    Reconnecting {
        attempt: u32,
    },
    Reconnected,
    /// HLS only: segments fell off the server's playlist before they could be fetched, so the audio
    /// jumps ahead. `None` when the server restarted its numbering and the amount is unknown.
    Skipped {
        segments: Option<u64>,
    },
    /// The stream is over for good, with the reason.
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Options {
    pub http: HttpConfig,
    /// The delay before the first reconnection attempt. Each further one waits twice as long.
    pub backoff: Duration,
    pub max_backoff: Duration,
    /// Consecutive attempts, each without receiving any audio, before the stream gives up.
    pub max_failures: u32,
    /// How often HLS playlists are polled. The playlist's own target duration when `None`.
    pub hls_poll: Option<Duration>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            http: HttpConfig::default(),
            backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(10),
            max_failures: 6,
            hls_poll: None,
        }
    }
}

impl Options {
    /// The wait before reconnection attempt number `attempt` (from 1).
    pub(crate) fn delay(&self, attempt: u32) -> Duration {
        self.backoff
            .saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)))
            .min(self.max_backoff)
    }
}

/// A station being listened to. Reading yields audio bytes only.
///
/// Dropping it, or calling [`cancel_handle`](Self::cancel_handle)'s `cancel`, ends the stream: a
/// plain-HTTP read in progress stops within a quarter of a second, an HTTPS one when its stall
/// timeout next fires.
pub struct LiveStream {
    reader: Box<dyn Read + Send>,
    format: Format,
    info: StationInfo,
    url: Url,
    events: Option<Receiver<Event>>,
    cancel: CancelHandle,
}

impl LiveStream {
    /// What the decoder should expect. Decided from the audio itself where possible, since servers
    /// often mislabel it.
    pub fn format(&self) -> Format {
        self.format
    }

    pub fn info(&self) -> &StationInfo {
        &self.info
    }

    /// The address the audio is actually coming from, after playlists and redirects.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The stream's events, once. Titles and reconnections are queued until taken, so none are
    /// lost between `open` and the first call.
    pub fn take_events(&mut self) -> Option<Receiver<Event>> {
        self.events.take()
    }

    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }
}

impl Read for LiveStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

impl Drop for LiveStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Connects to a station: a stream address, an HLS playlist, or a `.pls`/`.m3u` naming either.
pub fn open(url: &str) -> Result<LiveStream> {
    open_with(url, &Options::default())
}

/// A playlist's entries are tried in order and the first that connects is used.
pub fn open_with(url: &str, options: &Options) -> Result<LiveStream> {
    let url = parse_url(url)?;
    let cancel = CancelHandle::new();
    let http = Http::new(options.http.clone(), cancel.clone());
    let (events_tx, events_rx) = mpsc::channel();
    let opened = open_address(&http, &url, options, &events_tx, 0)?;
    Ok(LiveStream {
        reader: opened.reader,
        format: opened.format,
        info: opened.info,
        url: opened.url,
        events: Some(events_rx),
        cancel,
    })
}

struct Opened {
    reader: Box<dyn Read + Send>,
    format: Format,
    info: StationInfo,
    url: Url,
}

fn open_address(
    http: &Http,
    url: &Url,
    options: &Options,
    events: &Sender<Event>,
    depth: u32,
) -> Result<Opened> {
    match probe(http, url, true)? {
        Probed::Audio(response) => open_audio(http, url, response, options, events),
        Probed::Playlist {
            playlist: Playlist::Hls,
            text,
            base,
        } => {
            let (reader, format, media_url) =
                hls::start(http.clone(), &base, &text, options, events.clone())?;
            Ok(Opened {
                reader: Box::new(reader),
                format,
                info: StationInfo::default(),
                url: media_url,
            })
        }
        Probed::Playlist {
            playlist: Playlist::Entries(entries),
            ..
        } => {
            if depth >= MAX_PLAYLIST_DEPTH {
                return Err(Error::NotAStream(url.to_string()));
            }
            let mut last = Error::NotAStream(url.to_string());
            for entry in entries {
                match open_address(http, &entry, options, events, depth + 1) {
                    Ok(opened) => return Ok(opened),
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(error) => last = error,
                }
            }
            Err(last)
        }
    }
}

fn open_audio(
    http: &Http,
    url: &Url,
    response: Response,
    options: &Options,
    events: &Sender<Event>,
) -> Result<Opened> {
    let info = StationInfo::from_headers(&response.headers);
    let finite = response.headers.get("content-length").is_some()
        && response.headers.get("icy-name").is_none()
        && response.headers.get("icy-metaint").is_none();
    let media_format = response
        .headers
        .media_type()
        .as_deref()
        .and_then(format::from_media_type);
    let mut body = demux(response, 0, events);

    // The first bytes of audio, past any metadata, say what it is better than the server did.
    let mut prefix = Vec::new();
    let mut sniffed = None;
    while sniffed.is_none() && prefix.len() < SNIFF_LIMIT {
        let start = prefix.len();
        prefix.resize(start + SNIFF_STEP, 0);
        let read = body
            .read(&mut prefix[start..])
            .map_err(|error| crate::http::io_error(url, &error))?;
        prefix.truncate(start + read);
        if read == 0 {
            break;
        }
        sniffed = format::sniff(&prefix);
    }
    let format = sniffed
        .or(media_format)
        .ok_or_else(|| Error::UnknownFormat(url.to_string()))?;

    let reader = Persistent {
        http: http.clone(),
        url: url.clone(),
        options: options.clone(),
        events: events.clone(),
        prefix,
        prefix_read: 0,
        body,
        attempts: 0,
        finite,
    };
    Ok(Opened {
        reader: Box::new(reader),
        format,
        info,
        url: url.clone(),
    })
}

/// Wraps a response body so that its ICY metadata, if it has any, is removed and reported.
fn demux(response: Response, base: u64, events: &Sender<Event>) -> Demuxer<Box<dyn Read + Send>> {
    let metaint = response
        .headers
        .get("icy-metaint")
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    Demuxer::new(response.body, metaint, base, events.clone())
}

/// A stream that reconnects when its connection ends.
struct Persistent {
    http: Http,
    url: Url,
    options: Options,
    events: Sender<Event>,
    /// The audio read while finding out the format, handed out first.
    prefix: Vec<u8>,
    prefix_read: usize,
    body: Demuxer<Box<dyn Read + Send>>,
    /// Reconnection attempts since audio last arrived.
    attempts: u32,
    finite: bool,
}

impl Read for Persistent {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.prefix_read < self.prefix.len() {
            let count = buf.len().min(self.prefix.len() - self.prefix_read);
            buf[..count].copy_from_slice(&self.prefix[self.prefix_read..self.prefix_read + count]);
            self.prefix_read += count;
            return Ok(count);
        }
        loop {
            let cause = match self.body.read(buf) {
                Ok(0) if self.finite => return Ok(0),
                Ok(0) => io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the station closed the connection",
                ),
                Ok(count) => {
                    self.attempts = 0;
                    return Ok(count);
                }
                Err(error) if is_cancelled(&error) || self.finite => return Err(error),
                Err(error) => error,
            };
            self.reconnect(cause)?;
        }
    }
}

impl Persistent {
    fn reconnect(&mut self, mut cause: io::Error) -> io::Result<()> {
        loop {
            self.attempts += 1;
            if self.attempts > self.options.max_failures {
                // The listener is told once, by the error; the event is for interfaces that only
                // watch the channel.
                let _ = self.events.send(Event::Failed(cause.to_string()));
                return Err(cause);
            }
            let _ = self.events.send(Event::Reconnecting {
                attempt: self.attempts,
            });
            self.http
                .cancel_handle()
                .sleep(self.options.delay(self.attempts))
                .map_err(Error::into_io)?;
            match self.connect() {
                Ok(()) => {
                    let _ = self.events.send(Event::Reconnected);
                    return Ok(());
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled.into_io()),
                Err(error) => cause = error.into_io(),
            }
        }
    }

    fn connect(&mut self) -> Result<()> {
        let response = match probe(&self.http, &self.url, true)? {
            Probed::Audio(response) => response,
            Probed::Playlist { .. } => return Err(Error::NotAStream(self.url.to_string())),
        };
        self.body = demux(response, self.body.position(), &self.events);
        Ok(())
    }
}

#[cfg(test)]
mod hls_tests;
#[cfg(test)]
mod tests;
