//! Chromecasts and other Google Cast speakers, driven the way a sender app does: CASTV2, which is
//! length-prefixed protobuf messages carrying JSON over TLS on port 8009. The Default Media
//! Receiver app plays an HTTP link with no registration.
//!
//! Only the small part of the protocol that playing a link needs is spoken, and the one protobuf
//! message involved is written by hand, which keeps a protobuf runtime and its code generator out
//! of the build.

use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, ring};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use serde_json::{Value, json};

use crate::media::{Media, Receiver, State, Status};
use crate::{Error, Result};

pub(crate) const PORT: u16 = 8009;

/// The Default Media Receiver, which every Cast device has.
const MEDIA_RECEIVER: &str = "CC1AD845";

const SENDER: &str = "sender-0";
const PLATFORM: &str = "receiver-0";
const CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
const HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
const MEDIA: &str = "urn:x-cast:com.google.cast.media";

/// Cast messages are small; anything larger is not one.
const MAX_MESSAGE: usize = 1 << 20;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Starting the receiver app on a speaker that was idle takes a few seconds.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(20);
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// A speaker is pinged this often, and is gone if it says nothing at all for three times that.
const PING_EVERY: Duration = Duration::from_secs(5);
const SILENCE_LIMIT: Duration = Duration::from_secs(15);
/// How often the playback state is asked for, on top of what the speaker volunteers.
const ASK_EVERY: Duration = Duration::from_secs(2);
/// How long `status` waits for the next thing the speaker has to say.
const LISTEN: Duration = Duration::from_millis(20);
/// How long in all it listens: a speaker that never stops talking must not hold the driver up.
const LISTEN_BUDGET: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub source: String,
    pub destination: String,
    pub namespace: String,
    pub payload: String,
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_text(out: &mut Vec<u8>, field: u64, text: &str) {
    put_varint(out, field << 3 | 2);
    put_varint(out, text.len() as u64);
    out.extend_from_slice(text.as_bytes());
}

/// A `CastMessage` with its length in front, ready to send. Fields 1 and 5 are the protocol
/// version (CASTV2_1_0) and payload type (STRING), both zero.
pub(crate) fn encode(message: &Message) -> Vec<u8> {
    let mut body = vec![0x08, 0x00];
    put_text(&mut body, 2, &message.source);
    put_text(&mut body, 3, &message.destination);
    put_text(&mut body, 4, &message.namespace);
    body.extend_from_slice(&[0x28, 0x00]);
    put_text(&mut body, 6, &message.payload);
    let mut framed = (body.len() as u32).to_be_bytes().to_vec();
    framed.append(&mut body);
    framed
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Result<u64> {
    let mut value = 0_u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes
            .get(*at)
            .ok_or_else(|| Error::Protocol("a message ends mid-number".into()))?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(Error::Protocol("a number is too long".into()))
}

fn decode(bytes: &[u8]) -> Result<Message> {
    let mut message = Message {
        source: String::new(),
        destination: String::new(),
        namespace: String::new(),
        payload: String::new(),
    };
    let mut at = 0;
    while at < bytes.len() {
        let tag = read_varint(bytes, &mut at)?;
        match tag & 7 {
            0 => {
                read_varint(bytes, &mut at)?;
            }
            1 => at += 8,
            5 => at += 4,
            2 => {
                let length = read_varint(bytes, &mut at)? as usize;
                let end = at
                    .checked_add(length)
                    .filter(|&end| end <= bytes.len())
                    .ok_or_else(|| Error::Protocol("a field overruns its message".into()))?;
                let text = || String::from_utf8_lossy(&bytes[at..end]).into_owned();
                match tag >> 3 {
                    2 => message.source = text(),
                    3 => message.destination = text(),
                    4 => message.namespace = text(),
                    6 => message.payload = text(),
                    _ => {}
                }
                at = end;
            }
            wire => return Err(Error::Protocol(format!("unknown field type {wire}"))),
        }
    }
    Ok(message)
}

/// Cuts a stream of bytes into messages, however the bytes arrive.
#[derive(Default)]
pub(crate) struct Framer {
    buffered: Vec<u8>,
}

impl Framer {
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.buffered.extend_from_slice(bytes);
    }

    pub(crate) fn next(&mut self) -> Result<Option<Message>> {
        let Some(prefix) = self.buffered.first_chunk::<4>() else {
            return Ok(None);
        };
        let length = u32::from_be_bytes(*prefix) as usize;
        if length > MAX_MESSAGE {
            return Err(Error::Protocol(format!(
                "a message claims to be {length} bytes"
            )));
        }
        if self.buffered.len() < 4 + length {
            return Ok(None);
        }
        let message = decode(&self.buffered[4..4 + length]);
        self.buffered.drain(..4 + length);
        message.map(Some)
    }
}

// What is sent. Request ids tie a reply to its request.

fn connect() -> Value {
    json!({ "type": "CONNECT" })
}

fn launch(request_id: i64) -> Value {
    json!({ "type": "LAUNCH", "appId": MEDIA_RECEIVER, "requestId": request_id })
}

fn get_status(request_id: i64) -> Value {
    json!({ "type": "GET_STATUS", "requestId": request_id })
}

fn load(request_id: i64, session_id: &str, media: &Media, start_ms: u64, autoplay: bool) -> Value {
    let mut metadata = json!({ "metadataType": 3, "title": media.title, "artist": media.artist, "albumName": media.album });
    if let Some(art) = &media.art_url {
        metadata["images"] = json!([{ "url": art }]);
    }
    let mut item = json!({ "contentId": media.url, "contentType": media.mime, "streamType": "BUFFERED", "metadata": metadata });
    if media.duration_ms > 0 {
        item["duration"] = json!(media.duration_ms as f64 / 1000.0);
    }
    json!({
        "type": "LOAD", "requestId": request_id, "sessionId": session_id, "media": item,
        "autoplay": autoplay, "currentTime": start_ms as f64 / 1000.0,
    })
}

fn media_command(kind: &str, request_id: i64, media_session_id: i64) -> Value {
    json!({ "type": kind, "requestId": request_id, "mediaSessionId": media_session_id })
}

fn seek(request_id: i64, media_session_id: i64, position_ms: u64) -> Value {
    json!({ "type": "SEEK", "requestId": request_id, "mediaSessionId": media_session_id, "currentTime": position_ms as f64 / 1000.0 })
}

fn set_volume(request_id: i64, level: f32) -> Value {
    json!({ "type": "SET_VOLUME", "requestId": request_id, "volume": { "level": level } })
}

fn stop_app(request_id: i64, session_id: &str) -> Value {
    json!({ "type": "STOP", "requestId": request_id, "sessionId": session_id })
}

/// What the media receiver says about its playback, from a `MEDIA_STATUS` message. An empty list,
/// which it sends when nothing is loaded, is stopped.
#[derive(Clone, Debug, PartialEq)]
struct Playback {
    media_session_id: i64,
    status: Status,
}

fn parse_media_status(payload: &Value) -> Option<Playback> {
    let Some(entry) = payload["status"].as_array()?.first() else {
        return Some(Playback {
            media_session_id: 0,
            status: Status {
                state: State::Stopped,
                position_ms: 0,
                duration_ms: 0,
                volume: None,
                url: None,
                finished: false,
                error: None,
            },
        });
    };
    let idle_reason = entry["idleReason"].as_str();
    let state = match entry["playerState"].as_str()? {
        "PLAYING" => State::Playing,
        "PAUSED" => State::Paused,
        "BUFFERING" => State::Buffering,
        _ => State::Stopped,
    };
    let seconds = |value: &Value| (value.as_f64().unwrap_or(0.0).max(0.0) * 1000.0).round() as u64;
    Some(Playback {
        media_session_id: entry["mediaSessionId"].as_i64()?,
        status: Status {
            state,
            position_ms: seconds(&entry["currentTime"]),
            duration_ms: seconds(&entry["media"]["duration"]),
            volume: None,
            url: entry["media"]["contentId"].as_str().map(str::to_owned),
            finished: state == State::Stopped && idle_reason == Some("FINISHED"),
            error: (state == State::Stopped && idle_reason == Some("ERROR"))
                .then(|| "the speaker could not play the song".to_owned()),
        },
    })
}

/// The running media receiver app in a `RECEIVER_STATUS`: its session and the id to address it by.
fn parse_app(payload: &Value) -> Option<(String, String)> {
    let app = payload["status"]["applications"]
        .as_array()?
        .iter()
        .find(|app| app["appId"] == MEDIA_RECEIVER)?;
    Some((
        app["sessionId"].as_str()?.to_owned(),
        app["transportId"].as_str()?.to_owned(),
    ))
}

fn parse_volume(payload: &Value) -> Option<f32> {
    let volume = &payload["status"]["volume"];
    if volume["muted"] == true {
        return Some(0.0);
    }
    volume["level"].as_f64().map(|level| level as f32)
}

/// A byte stream with a read timeout: a socket, or a TLS session over one.
pub(crate) trait Wire: Read + Write + Send {
    fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()>;
}

impl Wire for TcpStream {
    fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        TcpStream::set_read_timeout(self, Some(timeout))
    }
}

impl Wire for StreamOwned<ClientConnection, TcpStream> {
    fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        self.sock.set_read_timeout(Some(timeout))
    }
}

/// Accepts the certificate a Cast device presents. Each has one of its own, signed by a Google
/// authority that is in no trust store, so there is nothing to check it against; what travels
/// over the connection is a link to a song and its controls.
#[derive(Debug)]
struct AnyDevice(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for AnyDevice {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

fn tls_config() -> Result<Arc<ClientConfig>> {
    let provider: CryptoProvider = ring::default_provider();
    let algorithms = provider.signature_verification_algorithms;
    let config = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .map_err(|error| Error::Protocol(format!("TLS setup failed: {error}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyDevice(algorithms)))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

pub(crate) fn connect_to(
    address: IpAddr,
    port: u16,
) -> Result<Chromecast<StreamOwned<ClientConnection, TcpStream>>> {
    connect_within(address, port, CONNECT_TIMEOUT)
}

/// Connects, giving the TCP connection and then the TLS handshake `timeout` each. A host that
/// accepts the connection and never answers would otherwise be waited on forever.
fn connect_within(
    address: IpAddr,
    port: u16,
    timeout: Duration,
) -> Result<Chromecast<StreamOwned<ClientConnection, TcpStream>>> {
    let socket = TcpStream::connect_timeout(&SocketAddr::new(address, port), timeout)
        .map_err(|error| Error::Lost(format!("can't reach the speaker: {error}")))?;
    socket.set_nodelay(true)?;
    socket.set_write_timeout(Some(REPLY_TIMEOUT))?;
    // The handshake happens inside the first write, and reads with this timeout.
    socket.set_read_timeout(Some(timeout))?;
    let connection = ClientConnection::new(tls_config()?, ServerName::IpAddress(address.into()))
        .map_err(|error| Error::Protocol(format!("TLS setup failed: {error}")))?;
    Chromecast::start(StreamOwned::new(connection, socket))
}

pub(crate) struct Chromecast<W: Wire> {
    wire: W,
    framer: Framer,
    requests: i64,
    session_id: String,
    transport_id: String,
    volume: Option<f32>,
    /// The latest playback state, and when it was reported.
    playback: Option<(Playback, Instant)>,
    /// The media session the last `LOAD` replaced. Until the new one reports, news of the old
    /// one (stopped, interrupted) is not news about the song just loaded.
    replaced: Option<i64>,
    heard: Instant,
    pinged: Instant,
    asked: Instant,
    /// Set once the speaker has closed our channel, e.g. because another app took it over.
    closed: bool,
}

impl<W: Wire> Chromecast<W> {
    /// Connects to the platform, starts the media receiver app and connects to it.
    pub(crate) fn start(wire: W) -> Result<Self> {
        let now = Instant::now();
        let mut cast = Self {
            wire,
            framer: Framer::default(),
            requests: 0,
            session_id: String::new(),
            transport_id: String::new(),
            volume: None,
            playback: None,
            replaced: None,
            heard: now,
            pinged: now,
            asked: now,
            closed: false,
        };
        cast.send(CONNECTION, PLATFORM, &connect())?;
        let request_id = cast.next_request();
        cast.send(RECEIVER, PLATFORM, &launch(request_id))?;
        // The first status after a launch may not list the app yet; wait for one that does.
        let deadline = Instant::now() + LAUNCH_TIMEOUT;
        let (session_id, transport_id) = loop {
            let message = cast
                .receive_until(deadline)?
                .ok_or_else(|| Error::Lost("the speaker did not start its player".into()))?;
            let payload = payload_of(&message);
            if payload["type"] == "LAUNCH_ERROR" {
                return Err(Error::Refused(
                    "the speaker would not start its player".into(),
                ));
            }
            if message.namespace == RECEIVER
                && payload["type"] == "RECEIVER_STATUS"
                && let Some(app) = parse_app(&payload)
            {
                break app;
            }
        };
        cast.session_id = session_id;
        cast.send(CONNECTION, &transport_id, &connect())?;
        cast.transport_id = transport_id;
        Ok(cast)
    }

    fn next_request(&mut self) -> i64 {
        self.requests += 1;
        self.requests
    }

    fn send(&mut self, namespace: &str, destination: &str, payload: &Value) -> Result<()> {
        let message = Message {
            source: SENDER.into(),
            destination: destination.into(),
            namespace: namespace.into(),
            payload: payload.to_string(),
        };
        self.wire
            .write_all(&encode(&message))
            .and_then(|()| self.wire.flush())
            .map_err(|error| match error.kind() {
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
                    Error::Lost("the speaker stopped answering".into())
                }
                _ => Error::Io(error),
            })
    }

    /// The next message, or `None` if none arrives within `wait`. Everything is also taken in
    /// here: pings are answered and playback and volume news is kept.
    fn receive(&mut self, wait: Duration) -> Result<Option<Message>> {
        self.receive_until(Instant::now() + wait)
    }

    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Message>> {
        loop {
            if let Some(message) = self.framer.next()? {
                self.heard = Instant::now();
                self.absorb(&message)?;
                return Ok(Some(message));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            self.wire
                .set_read_timeout((deadline - now).max(Duration::from_millis(1)))?;
            let mut chunk = [0_u8; 8192];
            match self.wire.read(&mut chunk) {
                Ok(0) => return Err(Error::Lost("the speaker hung up".into())),
                Ok(read) => self.framer.push(&chunk[..read]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(Error::Io(error)),
            }
        }
    }

    fn absorb(&mut self, message: &Message) -> Result<()> {
        let payload = payload_of(message);
        match (message.namespace.as_str(), payload["type"].as_str()) {
            (HEARTBEAT, Some("PING")) => {
                self.send(HEARTBEAT, &message.source, &json!({ "type": "PONG" }))?
            }
            (CONNECTION, Some("CLOSE")) => self.closed = true,
            (RECEIVER, Some("RECEIVER_STATUS")) => {
                if let Some(volume) = parse_volume(&payload) {
                    self.volume = Some(volume);
                }
            }
            (MEDIA, Some("MEDIA_STATUS")) => {
                if let Some(playback) = parse_media_status(&payload) {
                    // The old media session reporting that it was replaced says nothing of this song.
                    let stale = self
                        .replaced
                        .is_some_and(|old| old == playback.media_session_id);
                    if !stale {
                        self.replaced = None;
                        self.playback = Some((playback, Instant::now()));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Waits for the reply to request `request_id` on `namespace`. Anything else that arrives is
    /// taken in on the way.
    fn reply_to(&mut self, request_id: i64) -> Result<Value> {
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let message = self
                .receive_until(deadline)?
                .ok_or_else(|| Error::Lost("the speaker stopped answering".into()))?;
            if self.closed {
                return Err(Error::Lost("another app has taken over the speaker".into()));
            }
            let payload = payload_of(&message);
            if payload["requestId"].as_i64() != Some(request_id) {
                continue;
            }
            return match payload["type"].as_str() {
                Some(
                    "LOAD_FAILED" | "LOAD_CANCELLED" | "INVALID_REQUEST" | "INVALID_PLAYER_STATE",
                ) => Err(Error::Refused(format!(
                    "the speaker refused the request ({})",
                    payload["type"].as_str().unwrap_or_default()
                ))),
                _ => Ok(payload),
            };
        }
    }

    fn media_session(&self) -> Result<i64> {
        self.playback
            .as_ref()
            .map(|(playback, _)| playback.media_session_id)
            .filter(|&id| id != 0)
            .ok_or_else(|| Error::Refused("the speaker has nothing loaded".into()))
    }

    fn command(&mut self, kind: &str) -> Result<()> {
        let request_id = self.next_request();
        let session = self.media_session()?;
        let transport = self.transport_id.clone();
        self.send(MEDIA, &transport, &media_command(kind, request_id, session))?;
        self.reply_to(request_id).map(drop)
    }
}

fn payload_of(message: &Message) -> Value {
    serde_json::from_str(&message.payload).unwrap_or(Value::Null)
}

impl<W: Wire> Receiver for Chromecast<W> {
    fn load(&mut self, media: &Media, start_ms: u64, autoplay: bool) -> Result<()> {
        self.replaced = self
            .playback
            .as_ref()
            .map(|(playback, _)| playback.media_session_id)
            .filter(|&id| id != 0);
        self.playback = None;
        let request_id = self.next_request();
        let (session, transport) = (self.session_id.clone(), self.transport_id.clone());
        self.send(
            MEDIA,
            &transport,
            &load(request_id, &session, media, start_ms, autoplay),
        )?;
        self.reply_to(request_id)?;
        Ok(())
    }

    fn play(&mut self) -> Result<()> {
        self.command("PLAY")
    }

    fn pause(&mut self) -> Result<()> {
        self.command("PAUSE")
    }

    fn seek(&mut self, position_ms: u64) -> Result<()> {
        let request_id = self.next_request();
        let session = self.media_session()?;
        let transport = self.transport_id.clone();
        self.send(MEDIA, &transport, &seek(request_id, session, position_ms))?;
        self.reply_to(request_id).map(drop)
    }

    fn set_volume(&mut self, volume: f32) -> Result<()> {
        let level = volume.clamp(0.0, 1.0);
        let request_id = self.next_request();
        self.send(RECEIVER, PLATFORM, &set_volume(request_id, level))?;
        self.volume = Some(level);
        Ok(())
    }

    fn status(&mut self) -> Result<Status> {
        // Take in what the speaker has said since last time, then keep the conversation alive.
        let deadline = Instant::now() + LISTEN_BUDGET;
        while Instant::now() < deadline && self.receive(LISTEN)?.is_some() {}
        if self.closed {
            return Err(Error::Lost("another app has taken over the speaker".into()));
        }
        if self.heard.elapsed() > SILENCE_LIMIT {
            return Err(Error::Lost("the speaker stopped answering".into()));
        }
        if self.pinged.elapsed() >= PING_EVERY {
            self.pinged = Instant::now();
            self.send(HEARTBEAT, PLATFORM, &json!({ "type": "PING" }))?;
        }
        if self.asked.elapsed() >= ASK_EVERY && self.media_session().is_ok() {
            self.asked = Instant::now();
            let (request_id, transport) = (self.next_request(), self.transport_id.clone());
            self.send(MEDIA, &transport, &get_status(request_id))?;
        }
        let Some((playback, at)) = &self.playback else {
            return Ok(Status {
                state: State::Stopped,
                position_ms: 0,
                duration_ms: 0,
                volume: self.volume,
                url: None,
                finished: false,
                error: None,
            });
        };
        Ok(Status {
            volume: self.volume,
            ..playback.status.advanced(at.elapsed())
        })
    }

    fn stop(&mut self) -> Result<()> {
        if self.media_session().is_ok() {
            self.command("STOP")?;
        }
        Ok(())
    }

    /// Closes the player app, which sends the speaker back to what it was doing before.
    fn disconnect(&mut self) {
        let request_id = self.next_request();
        let session = self.session_id.clone();
        let _ = self.send(RECEIVER, PLATFORM, &stop_app(request_id, &session));
        let _ = self.send(CONNECTION, PLATFORM, &json!({ "type": "CLOSE" }));
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::Mutex;
    use std::thread;

    use super::*;

    fn message(payload: &str) -> Message {
        Message {
            source: SENDER.into(),
            destination: PLATFORM.into(),
            namespace: CONNECTION.into(),
            payload: payload.into(),
        }
    }

    fn song() -> Media {
        Media {
            url: "http://192.168.1.2:5000/link/abc.flac".into(),
            mime: "audio/flac".into(),
            title: "Song".into(),
            artist: "Band".into(),
            album: "Hits".into(),
            art_url: Some("http://192.168.1.2:5000/link/art.jpg".into()),
            duration_ms: 205_500,
        }
    }

    #[test]
    fn a_message_is_encoded_as_a_castmessage_with_its_length_in_front() {
        let framed = encode(&message("{\"type\":\"CONNECT\"}"));
        let mut expected = vec![0x08, 0x00, 0x12, 8];
        expected.extend_from_slice(b"sender-0");
        expected.extend_from_slice(&[0x1a, 10]);
        expected.extend_from_slice(b"receiver-0");
        expected.extend_from_slice(&[0x22, CONNECTION.len() as u8]);
        expected.extend_from_slice(CONNECTION.as_bytes());
        expected.extend_from_slice(&[0x28, 0x00, 0x32, 18]);
        expected.extend_from_slice(b"{\"type\":\"CONNECT\"}");
        assert_eq!(framed[..4], (expected.len() as u32).to_be_bytes());
        assert_eq!(framed[4..], expected[..]);
    }

    #[test]
    fn long_payloads_use_multi_byte_lengths_and_survive_a_round_trip() {
        let long = message(&format!("{{\"text\":\"{}\"}}", "é".repeat(500)));
        let framed = encode(&long);
        assert_eq!(decode(&framed[4..]).unwrap(), long);
    }

    #[test]
    fn unknown_fields_are_skipped() {
        let mut bytes = encode(&message("{}"))[4..].to_vec();
        bytes.extend_from_slice(&[0x38, 0x07, 0x42, 2, b'h', b'i']);
        assert_eq!(decode(&bytes).unwrap().payload, "{}");
    }

    #[test]
    fn truncated_and_overrunning_fields_are_errors_not_panics() {
        assert!(decode(&[0x12, 0x20, b'a']).is_err());
        assert!(decode(&[0x12]).is_err());
        assert!(decode(&[0x07]).is_err());
    }

    #[test]
    fn the_framer_waits_for_a_whole_message_whatever_the_chunking() {
        let first = encode(&message("{\"n\":1}"));
        let second = encode(&message("{\"n\":2}"));
        let mut framer = Framer::default();
        let wire: Vec<u8> = [first.clone(), second].concat();
        for byte in &wire[..first.len() - 1] {
            framer.push(&[*byte]);
            assert_eq!(framer.next().unwrap(), None);
        }
        framer.push(&wire[first.len() - 1..]);
        assert_eq!(framer.next().unwrap().unwrap().payload, "{\"n\":1}");
        assert_eq!(framer.next().unwrap().unwrap().payload, "{\"n\":2}");
        assert_eq!(framer.next().unwrap(), None);
    }

    #[test]
    fn the_framer_refuses_a_message_of_absurd_size() {
        let mut framer = Framer::default();
        framer.push(&u32::MAX.to_be_bytes());
        assert!(framer.next().is_err());
    }

    #[test]
    fn launch_names_the_default_media_receiver() {
        assert_eq!(
            launch(3),
            json!({ "type": "LAUNCH", "appId": "CC1AD845", "requestId": 3 })
        );
    }

    #[test]
    fn load_carries_the_link_the_details_and_where_to_start() {
        let body = load(7, "session-1", &song(), 65_250, true);
        assert_eq!(
            body,
            json!({
                "type": "LOAD", "requestId": 7, "sessionId": "session-1", "autoplay": true, "currentTime": 65.25,
                "media": {
                    "contentId": "http://192.168.1.2:5000/link/abc.flac", "contentType": "audio/flac", "streamType": "BUFFERED", "duration": 205.5,
                    "metadata": {
                        "metadataType": 3, "title": "Song", "artist": "Band", "albumName": "Hits",
                        "images": [{ "url": "http://192.168.1.2:5000/link/art.jpg" }]
                    }
                }
            })
        );
    }

    #[test]
    fn load_leaves_out_the_art_and_duration_it_does_not_have() {
        let bare = Media {
            art_url: None,
            duration_ms: 0,
            ..song()
        };
        let body = load(1, "s", &bare, 0, false);
        assert_eq!(body["autoplay"], false);
        assert!(body["media"].get("duration").is_none());
        assert!(body["media"]["metadata"].get("images").is_none());
    }

    #[test]
    fn transport_commands_name_the_media_session() {
        assert_eq!(
            media_command("PAUSE", 4, 9),
            json!({ "type": "PAUSE", "requestId": 4, "mediaSessionId": 9 })
        );
        assert_eq!(
            seek(5, 9, 30_500),
            json!({ "type": "SEEK", "requestId": 5, "mediaSessionId": 9, "currentTime": 30.5 })
        );
        assert_eq!(set_volume(6, 0.25)["volume"]["level"], 0.25);
    }

    #[test]
    fn media_status_gives_state_position_and_what_is_loaded() {
        let payload = json!({ "type": "MEDIA_STATUS", "status": [{
            "mediaSessionId": 3, "playerState": "PLAYING", "currentTime": 12.5,
            "media": { "contentId": "http://x/y.flac", "duration": 205.5 }
        }]});
        let playback = parse_media_status(&payload).unwrap();
        assert_eq!(playback.media_session_id, 3);
        assert_eq!(playback.status.state, State::Playing);
        assert_eq!(
            (playback.status.position_ms, playback.status.duration_ms),
            (12_500, 205_500)
        );
        assert_eq!(playback.status.url.as_deref(), Some("http://x/y.flac"));
    }

    #[test]
    fn a_song_that_played_out_is_finished_and_one_that_failed_is_an_error() {
        let idle = |reason: &str| json!({ "status": [{ "mediaSessionId": 3, "playerState": "IDLE", "idleReason": reason, "currentTime": 0.0 }]});
        let finished = parse_media_status(&idle("FINISHED")).unwrap().status;
        assert!(finished.finished && finished.error.is_none() && finished.state == State::Stopped);
        let failed = parse_media_status(&idle("ERROR")).unwrap().status;
        assert!(!failed.finished && failed.error.is_some());
        let interrupted = parse_media_status(&idle("INTERRUPTED")).unwrap().status;
        assert!(!interrupted.finished && interrupted.error.is_none());
    }

    #[test]
    fn an_empty_status_list_is_stopped() {
        let playback =
            parse_media_status(&json!({ "type": "MEDIA_STATUS", "status": [] })).unwrap();
        assert_eq!(playback.status.state, State::Stopped);
    }

    #[test]
    fn receiver_status_gives_the_app_and_the_volume() {
        let payload = json!({ "type": "RECEIVER_STATUS", "status": {
            "applications": [{ "appId": "CC1AD845", "sessionId": "sess", "transportId": "web-5" }],
            "volume": { "level": 0.4, "muted": false }
        }});
        assert_eq!(parse_app(&payload), Some(("sess".into(), "web-5".into())));
        assert_eq!(parse_volume(&payload), Some(0.4));
        let other = json!({ "status": { "applications": [{ "appId": "OTHER", "sessionId": "s", "transportId": "t" }] } });
        assert_eq!(parse_app(&other), None);
        assert_eq!(
            parse_volume(&json!({ "status": { "volume": { "level": 0.4, "muted": true } } })),
            Some(0.0)
        );
    }

    /// A Cast device on a local socket (without the TLS): it answers like one, and keeps what it was sent.
    struct FakeCast {
        sent: Arc<Mutex<Vec<Message>>>,
        wire: TcpStream,
        /// The speaker's end of the connection, to hang up on.
        speaker: TcpStream,
    }

    type Answer = Box<dyn FnMut(&Message, &Value) -> Vec<(String, Value)> + Send>;

    impl FakeCast {
        /// `answer` gets each message and its JSON and returns messages to send back, as (namespace, JSON).
        fn start(mut answer: Answer) -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let wire = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (mut server, _) = listener.accept().unwrap();
            let speaker = server.try_clone().unwrap();
            let sent: Arc<Mutex<Vec<Message>>> = Arc::default();
            let seen = Arc::clone(&sent);
            thread::spawn(move || {
                let mut framer = Framer::default();
                let mut chunk = [0_u8; 4096];
                loop {
                    match server.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(read) => framer.push(&chunk[..read]),
                    }
                    while let Ok(Some(message)) = framer.next() {
                        let payload = payload_of(&message);
                        seen.lock().unwrap().push(message.clone());
                        for (namespace, reply) in answer(&message, &payload) {
                            let back = Message {
                                source: message.destination.clone(),
                                destination: SENDER.into(),
                                namespace,
                                payload: reply.to_string(),
                            };
                            if server.write_all(&encode(&back)).is_err() {
                                return;
                            }
                        }
                    }
                }
            });
            Self {
                sent,
                wire,
                speaker,
            }
        }

        fn types(&self) -> Vec<String> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .map(|m| {
                    payload_of(m)["type"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect()
        }

        fn payload(&self, kind: &str) -> Value {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .map(payload_of)
                .find(|payload| payload["type"] == kind)
                .unwrap_or(Value::Null)
        }
    }

    fn receiver_status(request_id: i64) -> (String, Value) {
        (
            RECEIVER.into(),
            json!({ "type": "RECEIVER_STATUS", "requestId": request_id, "status": {
                "applications": [{ "appId": "CC1AD845", "sessionId": "sess-1", "transportId": "web-1" }],
                "volume": { "level": 0.6, "muted": false }
            }}),
        )
    }

    fn media_status(request_id: i64, state: &str, time: f64) -> (String, Value) {
        (
            MEDIA.into(),
            json!({ "type": "MEDIA_STATUS", "requestId": request_id, "status": [{
                "mediaSessionId": 1, "playerState": state, "currentTime": time,
                "media": { "contentId": "http://192.168.1.2:5000/link/abc.flac", "duration": 205.5 }
            }]}),
        )
    }

    /// Answers the way a speaker does for a whole listening session.
    fn speaker() -> Answer {
        Box::new(|message, payload| {
            let id = payload["requestId"].as_i64().unwrap_or(0);
            match (
                message.namespace.as_str(),
                payload["type"].as_str().unwrap_or_default(),
            ) {
                (RECEIVER, "LAUNCH") => vec![receiver_status(id)],
                (HEARTBEAT, "PING") => vec![(HEARTBEAT.into(), json!({ "type": "PONG" }))],
                (MEDIA, "LOAD") => vec![media_status(
                    id,
                    if payload["autoplay"] == true {
                        "PLAYING"
                    } else {
                        "PAUSED"
                    },
                    payload["currentTime"].as_f64().unwrap(),
                )],
                (MEDIA, "PAUSE") => vec![media_status(id, "PAUSED", 20.0)],
                (MEDIA, "PLAY") => vec![media_status(id, "PLAYING", 20.0)],
                (MEDIA, "SEEK") => vec![media_status(
                    id,
                    "PLAYING",
                    payload["currentTime"].as_f64().unwrap(),
                )],
                (MEDIA, "STOP") => vec![(
                    MEDIA.into(),
                    json!({ "type": "MEDIA_STATUS", "requestId": id, "status": [] }),
                )],
                _ => vec![],
            }
        })
    }

    #[test]
    fn starting_launches_the_media_receiver_and_connects_to_it() {
        let fake = FakeCast::start(speaker());
        let cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        assert_eq!(cast.transport_id, "web-1");
        assert_eq!(cast.session_id, "sess-1");
        assert_eq!(cast.volume, Some(0.6));
        thread::sleep(Duration::from_millis(50));
        assert_eq!(fake.types(), ["CONNECT", "LAUNCH", "CONNECT"]);
        let messages = fake.sent.lock().unwrap();
        assert_eq!(messages[0].destination, "receiver-0");
        assert_eq!(messages[2].destination, "web-1");
    }

    #[test]
    fn a_song_is_loaded_played_paused_and_sought_on_its_own_media_session() {
        let fake = FakeCast::start(speaker());
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();

        cast.load(&song(), 20_000, true).unwrap();
        let status = cast.status().unwrap();
        assert_eq!(status.state, State::Playing);
        assert!(status.position_ms >= 20_000 && status.position_ms < 21_000);
        assert_eq!(
            status.url.as_deref(),
            Some("http://192.168.1.2:5000/link/abc.flac")
        );
        assert_eq!(status.volume, Some(0.6));

        cast.pause().unwrap();
        assert_eq!(cast.status().unwrap().state, State::Paused);
        cast.play().unwrap();
        cast.seek(90_000).unwrap();
        assert!(cast.status().unwrap().position_ms >= 90_000);

        assert_eq!(fake.payload("LOAD")["sessionId"], "sess-1");
        assert_eq!(fake.payload("PAUSE")["mediaSessionId"], 1);
        assert_eq!(fake.payload("SEEK")["currentTime"], 90.0);
        let media_messages: Vec<_> = fake
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.namespace == MEDIA)
            .map(|m| m.destination.clone())
            .collect();
        assert!(
            media_messages
                .iter()
                .all(|destination| destination == "web-1")
        );
    }

    #[test]
    fn a_paused_song_stays_where_it_is() {
        let fake = FakeCast::start(speaker());
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        cast.load(&song(), 0, false).unwrap();
        let first = cast.status().unwrap();
        thread::sleep(Duration::from_millis(60));
        let second = cast.status().unwrap();
        assert_eq!((first.state, second.state), (State::Paused, State::Paused));
        assert_eq!(first.position_ms, second.position_ms);
    }

    #[test]
    fn volume_is_set_on_the_platform_not_the_app() {
        let fake = FakeCast::start(speaker());
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        cast.set_volume(0.3).unwrap();
        assert_eq!(cast.status().unwrap().volume, Some(0.3));
        thread::sleep(Duration::from_millis(50));
        let sent = fake.sent.lock().unwrap();
        let message = sent
            .iter()
            .find(|m| payload_of(m)["type"] == "SET_VOLUME")
            .unwrap();
        assert_eq!(
            (message.namespace.as_str(), message.destination.as_str()),
            (RECEIVER, "receiver-0")
        );
    }

    #[test]
    fn a_song_that_plays_out_reports_finished() {
        let fake = FakeCast::start(Box::new({
            let mut inner = speaker();
            move |message, payload| {
                if payload["type"] == "LOAD" {
                    let id = payload["requestId"].as_i64().unwrap();
                    return vec![
                        media_status(id, "PLAYING", 0.0),
                        (
                            MEDIA.into(),
                            json!({ "type": "MEDIA_STATUS", "requestId": 0, "status": [{
                                "mediaSessionId": 1, "playerState": "IDLE", "idleReason": "FINISHED", "currentTime": 205.5,
                                "media": { "contentId": "http://192.168.1.2:5000/link/abc.flac", "duration": 205.5 }
                            }]}),
                        ),
                    ];
                }
                inner(message, payload)
            }
        }));
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        cast.load(&song(), 0, true).unwrap();
        thread::sleep(Duration::from_millis(60));
        let status = cast.status().unwrap();
        assert!(status.finished && status.state == State::Stopped);
    }

    #[test]
    fn the_old_songs_interruption_is_not_news_about_the_new_one() {
        let loads = Arc::new(Mutex::new(0));
        let fake = FakeCast::start(Box::new({
            let mut inner = speaker();
            let loads = Arc::clone(&loads);
            move |message, payload| {
                if payload["type"] != "LOAD" {
                    return inner(message, payload);
                }
                let id = payload["requestId"].as_i64().unwrap();
                let mut count = loads.lock().unwrap();
                *count += 1;
                if *count == 1 {
                    return vec![media_status(id, "PLAYING", 5.0)];
                }
                // The second load: the first session reports being replaced before the new one starts.
                let interrupted = (
                    MEDIA.into(),
                    json!({ "type": "MEDIA_STATUS", "requestId": 0, "status": [{
                    "mediaSessionId": 1, "playerState": "IDLE", "idleReason": "INTERRUPTED", "currentTime": 5.0 }]}),
                );
                let started = (
                    MEDIA.into(),
                    json!({ "type": "MEDIA_STATUS", "requestId": id, "status": [{
                    "mediaSessionId": 2, "playerState": "PLAYING", "currentTime": 0.0, "media": { "contentId": "second", "duration": 100.0 } }]}),
                );
                vec![interrupted, started]
            }
        }));
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        cast.load(&song(), 0, true).unwrap();
        cast.load(
            &Media {
                url: "second".into(),
                ..song()
            },
            0,
            true,
        )
        .unwrap();
        thread::sleep(Duration::from_millis(60));
        let status = cast.status().unwrap();
        assert_eq!(
            (status.state, status.url.as_deref(), status.finished),
            (State::Playing, Some("second"), false)
        );
    }

    #[test]
    fn pings_are_answered_with_pongs() {
        let fake = FakeCast::start(speaker());
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        let ping = Message {
            source: PLATFORM.into(),
            destination: SENDER.into(),
            namespace: HEARTBEAT.into(),
            payload: "{\"type\":\"PING\"}".into(),
        };
        cast.absorb(&ping).unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(fake.types().contains(&"PONG".to_owned()));
    }

    #[test]
    fn a_speaker_that_hangs_up_is_lost() {
        let fake = FakeCast::start(speaker());
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        fake.speaker.shutdown(std::net::Shutdown::Both).unwrap();
        let error = loop {
            match cast.status() {
                Ok(_) => thread::sleep(Duration::from_millis(10)),
                Err(error) => break error,
            }
        };
        assert!(error.is_lost(), "{error}");
    }

    #[test]
    fn an_app_that_took_the_speaker_over_is_lost() {
        let fake = FakeCast::start(Box::new({
            let mut inner = speaker();
            move |message, payload| {
                if payload["type"] == "LOAD" {
                    return vec![(CONNECTION.into(), json!({ "type": "CLOSE" }))];
                }
                inner(message, payload)
            }
        }));
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        assert!(cast.load(&song(), 0, true).unwrap_err().is_lost());
    }

    #[test]
    fn a_load_the_speaker_rejects_is_refused_not_lost() {
        let fake = FakeCast::start(Box::new({
            let mut inner = speaker();
            move |message, payload| {
                if payload["type"] == "LOAD" {
                    return vec![(
                        MEDIA.into(),
                        json!({ "type": "LOAD_FAILED", "requestId": payload["requestId"] }),
                    )];
                }
                inner(message, payload)
            }
        }));
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        let error = cast.load(&song(), 0, true).unwrap_err();
        assert!(matches!(error, Error::Refused(_)) && !error.is_lost());
    }

    #[test]
    fn disconnecting_stops_the_app() {
        let fake = FakeCast::start(speaker());
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        cast.disconnect();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(fake.payload("STOP")["sessionId"], "sess-1");
    }

    #[test]
    fn a_speaker_that_never_stops_talking_does_not_hold_up_the_status() {
        let fake = FakeCast::start(speaker());
        let mut cast = Chromecast::start(fake.wire.try_clone().unwrap()).unwrap();
        let mut chatter = fake.speaker.try_clone().unwrap();
        let talking = thread::spawn(move || {
            let noise = Message {
                source: "web-1".into(),
                destination: SENDER.into(),
                namespace: MEDIA.into(),
                payload: "{\"type\":\"NOISE\"}".into(),
            };
            let until = Instant::now() + Duration::from_millis(1500);
            while Instant::now() < until && chatter.write_all(&encode(&noise)).is_ok() {
                thread::sleep(Duration::from_micros(200));
            }
        });
        thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        cast.status().unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(700),
            "{:?}",
            started.elapsed()
        );
        talking.join().unwrap();
    }

    #[test]
    fn a_host_that_accepts_the_connection_and_never_answers_the_handshake_is_given_up_on() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let silent = thread::spawn(move || {
            // Holds the connection open, saying nothing, until the test is done with it.
            let held = listener.accept().unwrap();
            thread::sleep(Duration::from_secs(3));
            drop(held);
        });
        let started = Instant::now();
        let error = connect_within(Ipv4Addr::LOCALHOST.into(), port, Duration::from_millis(300))
            .err()
            .expect("no speaker answered");
        assert!(error.is_lost(), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        drop(silent);
    }

    #[test]
    fn the_tls_setup_builds() {
        assert!(tls_config().is_ok());
    }
}
