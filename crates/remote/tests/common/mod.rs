//! A stand-in Subsonic server on a local port. It checks the token like a real one does, logs
//! what it was asked, and can misbehave on demand when serving songs.
//!
//! It speaks HTTP over a plain `TcpListener` rather than through an HTTP server library, because
//! the misbehaviour needs control of the socket: breaking the connection in the middle of a body
//! and releasing a body a few bytes at a time, unbuffered, are things server libraries are built
//! to prevent.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;

use md5::{Digest, Md5};
use serde_json::{Value, json};

pub const USER: &str = "alice";
pub const PASSWORD: &str = "sesame";

#[derive(Clone, Debug)]
pub struct Logged {
    pub endpoint: String,
    pub params: HashMap<String, String>,
    pub range: Option<String>,
    pub raw_url: String,
}

struct Song {
    data: Vec<u8>,
    content_type: String,
}

/// How the server treats requests for songs.
#[derive(Default)]
pub struct Mode {
    /// Answer `Range` requests with 206/416 instead of ignoring them.
    pub honor_range: bool,
    /// Send this many bytes of the body, then break the connection.
    pub cut_after: Option<usize>,
    /// Send the body chunked, without a `Content-Length`.
    pub chunked: bool,
    /// Hold the body back: each number received lets that many more bytes out; closing the
    /// channel breaks the connection.
    pub gate: Option<Receiver<usize>>,
    /// Report a missing song as a bare HTTP 404 (as AntiStreaming does) rather than as a Subsonic
    /// error document (as Navidrome does).
    pub missing_as_404: bool,
}

#[derive(Default)]
struct State {
    songs: Mutex<HashMap<String, Song>>,
    payloads: Mutex<HashMap<String, Value>>,
    failures: Mutex<HashMap<String, (u32, String)>>,
    raw_replies: Mutex<HashMap<String, (u16, String, String)>>,
    covers: Mutex<HashMap<String, Vec<u8>>>,
    requests: Mutex<Vec<Logged>>,
    mode: Mutex<Mode>,
}

pub struct FakeServer {
    pub url: String,
    state: Arc<State>,
}

impl FakeServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(State::default());
        let shared = Arc::clone(&state);
        // Lives until the test process ends, like any server a test starts and forgets.
        thread::spawn(move || {
            for stream in listener.incoming().filter_map(Result::ok) {
                let shared = Arc::clone(&shared);
                // One thread each, so a held-back song does not block the other requests.
                thread::spawn(move || {
                    let _ = serve(&shared, stream);
                });
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            state,
        }
    }

    /// What `endpoint` answers with: the members of the `subsonic-response` besides its status.
    pub fn respond(&self, endpoint: &str, payload: Value) {
        self.state
            .payloads
            .lock()
            .unwrap()
            .insert(endpoint.to_owned(), payload);
    }

    pub fn fail(&self, endpoint: &str, code: u32, message: &str) {
        self.state
            .failures
            .lock()
            .unwrap()
            .insert(endpoint.to_owned(), (code, message.to_owned()));
    }

    /// Answers `endpoint` with this exact status, content type and body, ignoring credentials.
    pub fn reply_raw(&self, endpoint: &str, status: u16, content_type: &str, body: &str) {
        self.state.raw_replies.lock().unwrap().insert(
            endpoint.to_owned(),
            (status, content_type.to_owned(), body.to_owned()),
        );
    }

    pub fn song(&self, id: &str, data: Vec<u8>, content_type: &str) {
        let song = Song {
            data,
            content_type: content_type.to_owned(),
        };
        self.state.songs.lock().unwrap().insert(id.to_owned(), song);
    }

    pub fn cover(&self, id: &str, data: Vec<u8>) {
        self.state
            .covers
            .lock()
            .unwrap()
            .insert(id.to_owned(), data);
    }

    pub fn mode(&self, change: impl FnOnce(&mut Mode)) {
        change(&mut self.state.mode.lock().unwrap());
    }

    pub fn requests(&self) -> Vec<Logged> {
        self.state.requests.lock().unwrap().clone()
    }

    pub fn requests_to(&self, endpoint: &str) -> Vec<Logged> {
        self.requests()
            .into_iter()
            .filter(|request| request.endpoint == endpoint)
            .collect()
    }
}

/// `len` bytes that differ from place to place, so a misplaced chunk shows up in a comparison.
pub fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| {
            (i as u8)
                .wrapping_mul(31)
                .wrapping_add((i >> 8) as u8)
                .wrapping_add(seed)
        })
        .collect()
}

/// What to send back.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Body,
}

enum Body {
    Bytes(Vec<u8>),
    /// Streamed as it is read. `Some` length is declared up front; `None` is sent chunked.
    Stream(Box<dyn Read + Send>, Option<usize>),
}

impl Reply {
    fn new(status: u16, content_type: &str, body: Body) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_owned(), content_type.to_owned())],
            body,
        }
    }

    fn json(body: Value) -> Self {
        Self::new(
            200,
            "application/json; charset=utf-8",
            Body::Bytes(body.to_string().into_bytes()),
        )
    }

    fn bare(status: u16) -> Self {
        Self::new(status, "text/plain", Body::Bytes(Vec::new()))
    }

    fn with(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

fn serve(state: &State, stream: TcpStream) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let raw_url = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_owned();
    let mut range = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("range")
        {
            range = Some(value.trim().to_owned());
        }
    }

    let reply = handle(state, &raw_url, range);
    send(stream, reply)
}

fn handle(state: &State, raw_url: &str, range: Option<String>) -> Reply {
    let (path, query) = raw_url.split_once('?').unwrap_or((raw_url, ""));
    let endpoint = path
        .strip_prefix("/rest/")
        .map(|name| name.strip_suffix(".view").unwrap_or(name).to_owned())
        .unwrap_or_default();
    let params: HashMap<String, String> = form_urlencoded::parse(query.as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    state.requests.lock().unwrap().push(Logged {
        endpoint: endpoint.clone(),
        params: params.clone(),
        range: range.clone(),
        raw_url: raw_url.to_owned(),
    });

    if let Some((status, content_type, body)) = state.raw_replies.lock().unwrap().get(&endpoint) {
        return Reply::new(
            *status,
            content_type,
            Body::Bytes(body.clone().into_bytes()),
        );
    }
    if !authentic(&params) {
        return Reply::json(error_envelope(40, "Wrong username or password"));
    }
    if let Some((code, message)) = state.failures.lock().unwrap().get(&endpoint) {
        return Reply::json(error_envelope(*code, message));
    }
    match endpoint.as_str() {
        "stream" => song_reply(state, &params, range.as_deref()),
        "getCoverArt" => cover_reply(state, &params),
        _ => {
            let payload = state.payloads.lock().unwrap().get(&endpoint).cloned();
            Reply::json(ok_envelope(payload.unwrap_or_else(|| json!({}))))
        }
    }
}

/// The token check from the Subsonic protocol: `t` must be md5(password + `s`).
fn authentic(params: &HashMap<String, String>) -> bool {
    let (Some(user), Some(token), Some(salt)) = (params.get("u"), params.get("t"), params.get("s"))
    else {
        return false;
    };
    let expected = hex::encode(Md5::digest(format!("{PASSWORD}{salt}")));
    user == USER && *token == expected && !params.contains_key("p")
}

fn cover_reply(state: &State, params: &HashMap<String, String>) -> Reply {
    let id = params.get("id").map_or("", String::as_str);
    match state.covers.lock().unwrap().get(id) {
        Some(bytes) => Reply::new(200, "image/jpeg", Body::Bytes(bytes.clone())),
        None => missing(state),
    }
}

fn song_reply(state: &State, params: &HashMap<String, String>, range: Option<&str>) -> Reply {
    let id = params.get("id").map_or("", String::as_str);
    let (data, content_type) = match state.songs.lock().unwrap().get(id) {
        Some(song) => (song.data.clone(), song.content_type.clone()),
        None => return missing(state),
    };
    let mut mode = state.mode.lock().unwrap();
    let total = data.len();
    let mut start = 0;
    let mut reply = Reply::new(200, &content_type, Body::Bytes(Vec::new()));
    if mode.honor_range {
        reply = reply.with("Accept-Ranges", "bytes");
        if let Some(requested) = range.and_then(parse_range) {
            if requested >= total {
                return Reply::bare(416).with("Content-Range", &format!("bytes */{total}"));
            }
            start = requested;
            reply.status = 206;
            reply = reply.with(
                "Content-Range",
                &format!("bytes {start}-{}/{total}", total - 1),
            );
        }
    }
    let body = data[start..].to_vec();
    let length = body.len();
    let reader: Box<dyn Read + Send> = match (mode.gate.take(), mode.cut_after) {
        (Some(permits), _) => Box::new(GatedBody {
            data: body,
            position: 0,
            released: 0,
            permits,
        }),
        (None, Some(limit)) => Box::new(CutBody {
            data: body,
            position: 0,
            limit,
        }),
        (None, None) => Box::new(io::Cursor::new(body)),
    };
    reply.body = Body::Stream(reader, (!mode.chunked).then_some(length));
    reply
}

fn missing(state: &State) -> Reply {
    if state.mode.lock().unwrap().missing_as_404 {
        Reply::bare(404)
    } else {
        Reply::json(error_envelope(70, "The requested data was not found"))
    }
}

fn parse_range(value: &str) -> Option<usize> {
    value
        .strip_prefix("bytes=")?
        .strip_suffix('-')?
        .parse()
        .ok()
}

fn ok_envelope(payload: Value) -> Value {
    let mut response = json!({ "status": "ok", "version": "1.16.1", "openSubsonic": true });
    if let (Some(members), Some(extra)) = (response.as_object_mut(), payload.as_object()) {
        members.extend(extra.clone());
    }
    json!({ "subsonic-response": response })
}

fn error_envelope(code: u32, message: &str) -> Value {
    json!({ "subsonic-response": {
        "status": "failed",
        "version": "1.16.1",
        "error": { "code": code, "message": message },
    }})
}

/// Writes the reply and closes the connection. A body is written as it is produced, unbuffered,
/// so the client sees exactly what the test has released; if reading the body fails, the
/// connection is dropped there, mid-body, as a network failure would.
fn send(mut stream: TcpStream, reply: Reply) -> io::Result<()> {
    let reason = match reply.status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        _ => "Status",
    };
    let mut head = format!("HTTP/1.1 {} {reason}\r\n", reply.status);
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    match &reply.body {
        Body::Bytes(bytes) => head.push_str(&format!("Content-Length: {}\r\n", bytes.len())),
        Body::Stream(_, Some(length)) => head.push_str(&format!("Content-Length: {length}\r\n")),
        Body::Stream(_, None) => head.push_str("Transfer-Encoding: chunked\r\n"),
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;

    match reply.body {
        Body::Bytes(bytes) => stream.write_all(&bytes),
        Body::Stream(mut reader, length) => {
            let mut buffer = vec![0; 16 * 1024];
            loop {
                let read = reader.read(&mut buffer)?;
                if length.is_some() {
                    stream.write_all(&buffer[..read])?;
                } else if read > 0 {
                    stream.write_all(format!("{read:x}\r\n").as_bytes())?;
                    stream.write_all(&buffer[..read])?;
                    stream.write_all(b"\r\n")?;
                } else {
                    return stream.write_all(b"0\r\n\r\n");
                }
                stream.flush()?;
                if read == 0 {
                    return Ok(());
                }
            }
        }
    }
}

/// Yields its bytes only as many as the test has released, to play a slow connection.
struct GatedBody {
    data: Vec<u8>,
    position: usize,
    released: usize,
    permits: Receiver<usize>,
}

impl Read for GatedBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.data.len() {
            return Ok(0);
        }
        while self.position >= self.released {
            match self.permits.recv() {
                Ok(more) => self.released += more,
                // The test hung up: break the connection mid-song.
                Err(_) => return Err(io::Error::other("connection lost")),
            }
        }
        let end = self.released.min(self.data.len());
        let count = (end - self.position).min(buffer.len());
        buffer[..count].copy_from_slice(&self.data[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

/// Yields `limit` bytes and then fails.
struct CutBody {
    data: Vec<u8>,
    position: usize,
    limit: usize,
}

impl Read for CutBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.data.len() {
            return Ok(0);
        }
        if self.position >= self.limit {
            return Err(io::Error::other("connection lost"));
        }
        let end = self.limit.min(self.data.len());
        let count = (end - self.position).min(buffer.len());
        buffer[..count].copy_from_slice(&self.data[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}
