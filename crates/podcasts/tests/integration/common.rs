use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use antistreaming_podcasts::{Feed, Podcasts};
use tempfile::TempDir;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

pub fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap()
}

/// Deterministic stand-in for audio; the length seeds the pattern so files differ by size.
pub fn audio_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 7 + len) % 251) as u8).collect()
}

#[derive(Clone)]
enum Route {
    Resource {
        body: Vec<u8>,
        content_type: String,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    /// A body that goes out slowly or stops partway, to exercise stall detection.
    Streamed {
        body: Vec<u8>,
        pacing: Pacing,
        etag: Option<String>,
    },
    Redirect(String),
    Status(u16),
}

#[derive(Clone, Copy)]
enum Pacing {
    /// Sends this many bytes, then goes silent until the server shuts down.
    StallAfter(usize),
    /// Sends `chunk` bytes at a time with `delay` between them.
    Trickle { chunk: usize, delay: Duration },
}

/// Lets blocked responses finish when the server is dropped.
struct Release {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Release {
    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }

    fn wait(&self, at_most: Duration) {
        let open = self.open.lock().unwrap();
        let _ = self
            .changed
            .wait_timeout_while(open, at_most, |open| !*open);
    }
}

struct PacedReader {
    data: Vec<u8>,
    position: usize,
    pacing: Pacing,
    state: Arc<State>,
}

impl Read for PacedReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let allowed = match self.pacing {
            Pacing::StallAfter(limit) => {
                if self.position >= limit {
                    self.state.release.wait(Duration::from_secs(10));
                    return Ok(0);
                }
                limit - self.position
            }
            Pacing::Trickle { chunk, delay } => {
                std::thread::sleep(delay);
                chunk
            }
        };
        let count = buf.len().min(allowed).min(self.data.len() - self.position);
        buf[..count].copy_from_slice(&self.data[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

#[derive(Debug, Clone)]
pub struct Logged {
    pub path: String,
    pub range: Option<String>,
    pub if_range: Option<String>,
    pub if_none_match: Option<String>,
    pub if_modified_since: Option<String>,
    pub user_agent: Option<String>,
}

struct State {
    routes: Mutex<HashMap<String, Route>>,
    requests: Mutex<Vec<Logged>>,
    ranges: AtomicBool,
    announce_length: AtomicBool,
    release: Release,
}

pub struct TestServer {
    pub base: String,
    state: Arc<State>,
    server: Arc<Server>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    pub fn start() -> TestServer {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let state = Arc::new(State {
            routes: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
            ranges: AtomicBool::new(true),
            announce_length: AtomicBool::new(true),
            release: Release {
                open: Mutex::new(false),
                changed: Condvar::new(),
            },
        });
        let thread = {
            let (server, state) = (Arc::clone(&server), Arc::clone(&state));
            std::thread::spawn(move || {
                // A response that stalls on purpose must not hold up the requests behind it.
                for request in server.incoming_requests() {
                    let state = Arc::clone(&state);
                    std::thread::spawn(move || handle(&state, request));
                }
            })
        };
        TestServer {
            base: format!("http://{addr}"),
            state,
            server,
            thread: Some(thread),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Serves a fixture (with `{{BASE}}` filled in) as an RSS/Atom document.
    pub fn serve_fixture(&self, path: &str, name: &str) {
        self.serve_feed(path, &fixture(name));
    }

    pub fn serve_feed(&self, path: &str, xml: &str) {
        self.serve(
            path,
            xml.replace("{{BASE}}", &self.base).into_bytes(),
            "application/rss+xml",
            None,
            None,
        );
    }

    pub fn serve_feed_with_etag(&self, path: &str, name: &str, etag: &str) {
        let body = fixture(name).replace("{{BASE}}", &self.base).into_bytes();
        self.serve(path, body, "application/rss+xml", Some(etag), None);
    }

    pub fn serve_feed_with_last_modified(&self, path: &str, name: &str, last_modified: &str) {
        let body = fixture(name).replace("{{BASE}}", &self.base).into_bytes();
        self.serve(path, body, "application/rss+xml", None, Some(last_modified));
    }

    /// Audio with an ETag derived from its length, so a different length reads as a new file.
    pub fn serve_audio(&self, path: &str, len: usize) {
        let etag = format!("\"audio-{len}\"");
        self.serve(path, audio_bytes(len), "audio/mpeg", Some(&etag), None);
    }

    pub fn serve_audio_with(
        &self,
        path: &str,
        len: usize,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) {
        self.serve(path, audio_bytes(len), "audio/mpeg", etag, last_modified);
    }

    /// Sends `after` bytes of the audio and then nothing, until the server is dropped.
    pub fn serve_stalling_audio(&self, path: &str, len: usize, after: usize) {
        self.serve_streamed(path, len, Pacing::StallAfter(after));
    }

    /// Sends the audio in `chunk`-byte pieces with `delay` between them.
    pub fn serve_trickling_audio(&self, path: &str, len: usize, chunk: usize, delay: Duration) {
        self.serve_streamed(path, len, Pacing::Trickle { chunk, delay });
    }

    fn serve_streamed(&self, path: &str, len: usize, pacing: Pacing) {
        let route = Route::Streamed {
            body: audio_bytes(len),
            pacing,
            etag: Some(format!("\"audio-{len}\"")),
        };
        self.state
            .routes
            .lock()
            .unwrap()
            .insert(path.to_owned(), route);
    }

    fn serve(
        &self,
        path: &str,
        body: Vec<u8>,
        content_type: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) {
        let route = Route::Resource {
            body,
            content_type: content_type.to_owned(),
            etag: etag.map(str::to_owned),
            last_modified: last_modified.map(str::to_owned),
        };
        self.state
            .routes
            .lock()
            .unwrap()
            .insert(path.to_owned(), route);
    }

    pub fn redirect(&self, path: &str, target: &str) {
        self.state
            .routes
            .lock()
            .unwrap()
            .insert(path.to_owned(), Route::Redirect(target.to_owned()));
    }

    pub fn fail(&self, path: &str, status: u16) {
        self.state
            .routes
            .lock()
            .unwrap()
            .insert(path.to_owned(), Route::Status(status));
    }

    pub fn support_ranges(&self, enabled: bool) {
        self.state.ranges.store(enabled, Ordering::Relaxed);
    }

    /// When off, large bodies go out chunked, so clients see no Content-Length.
    pub fn announce_length(&self, enabled: bool) {
        self.state.announce_length.store(enabled, Ordering::Relaxed);
    }

    pub fn requests_for(&self, path: &str) -> Vec<Logged> {
        let requests = self.state.requests.lock().unwrap();
        requests
            .iter()
            .filter(|logged| logged.path == path)
            .cloned()
            .collect()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.state.release.open();
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn request_header(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.to_string())
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).unwrap()
}

fn handle(state: &Arc<State>, request: Request) {
    let path = request.url().to_owned();
    let logged = Logged {
        path: path.clone(),
        range: request_header(&request, "Range"),
        if_range: request_header(&request, "If-Range"),
        if_none_match: request_header(&request, "If-None-Match"),
        if_modified_since: request_header(&request, "If-Modified-Since"),
        user_agent: request_header(&request, "User-Agent"),
    };
    state.requests.lock().unwrap().push(logged.clone());

    let route = state.routes.lock().unwrap().get(&path).cloned();
    match (request.method(), route) {
        (
            Method::Get,
            Some(Route::Resource {
                body,
                content_type,
                etag,
                last_modified,
            }),
        ) => {
            let response = resource_response(
                &logged,
                body,
                &content_type,
                etag,
                last_modified,
                state.ranges.load(Ordering::Relaxed),
            );
            send(state, request, response);
        }
        (Method::Get, Some(Route::Streamed { body, pacing, etag })) => {
            let len = body.len();
            let reader = PacedReader {
                data: body,
                position: 0,
                pacing,
                state: Arc::clone(state),
            };
            let mut headers = vec![header("Content-Type", "audio/mpeg")];
            headers.extend(etag.map(|etag| header("ETag", &etag)));
            send(
                state,
                request,
                Response::new(StatusCode(200), headers, reader, Some(len), None),
            );
        }
        (Method::Get, Some(Route::Redirect(target))) => {
            let response = Response::from_data(Vec::new())
                .with_status_code(301)
                .with_header(header("Location", &target));
            send(state, request, response);
        }
        (Method::Get, Some(Route::Status(code))) => {
            send(
                state,
                request,
                Response::from_data(Vec::new()).with_status_code(code),
            );
        }
        _ => send(
            state,
            request,
            Response::from_data(Vec::new()).with_status_code(404),
        ),
    }
}

fn send<R: Read>(state: &State, request: Request, response: Response<R>) {
    // tiny_http switches to chunked encoding above 32 KiB unless told otherwise.
    let threshold = if state.announce_length.load(Ordering::Relaxed) {
        usize::MAX
    } else {
        32 * 1024
    };
    let _ = request.respond(response.with_chunked_threshold(threshold));
}

fn resource_response(
    logged: &Logged,
    body: Vec<u8>,
    content_type: &str,
    etag: Option<String>,
    last_modified: Option<String>,
    ranges: bool,
) -> Response<Cursor<Vec<u8>>> {
    let unchanged = (etag.is_some() && logged.if_none_match == etag)
        || (last_modified.is_some() && logged.if_modified_since == last_modified);
    let mut validators = Vec::new();
    if let Some(etag) = &etag {
        validators.push(header("ETag", etag));
    }
    if let Some(last_modified) = &last_modified {
        validators.push(header("Last-Modified", last_modified));
    }
    if unchanged {
        let response = Response::from_data(Vec::new()).with_status_code(304);
        return validators.into_iter().fold(response, Response::with_header);
    }

    let len = body.len();
    // A mismatched If-Range means the file changed, so the Range must be ignored.
    let range_applies = ranges
        && logged.if_range.as_ref().is_none_or(|token| {
            Some(token) == etag.as_ref() || Some(token) == last_modified.as_ref()
        });
    let start = logged
        .range
        .as_deref()
        .filter(|_| range_applies)
        .and_then(|range| {
            range
                .strip_prefix("bytes=")?
                .trim_end_matches('-')
                .parse::<usize>()
                .ok()
        });
    let response = match start {
        Some(start) if start >= len => Response::from_data(Vec::new())
            .with_status_code(416)
            .with_header(header("Content-Range", &format!("bytes */{len}"))),
        Some(start) => Response::from_data(body[start..].to_vec())
            .with_status_code(206)
            .with_header(header(
                "Content-Range",
                &format!("bytes {start}-{}/{len}", len - 1),
            )),
        None => Response::from_data(body).with_status_code(StatusCode(200)),
    };
    let response = response.with_header(header("Content-Type", content_type));
    let response = if ranges {
        response.with_header(header("Accept-Ranges", "bytes"))
    } else {
        response
    };
    validators.into_iter().fold(response, Response::with_header)
}

/// A server, a database and a download folder that live as long as the test.
pub struct Env {
    pub dir: TempDir,
    pub server: TestServer,
    pub podcasts: Podcasts,
}

impl Env {
    pub fn new() -> Env {
        let dir = TempDir::new().unwrap();
        let podcasts = Podcasts::open(
            &dir.path().join("podcasts.db"),
            &dir.path().join("downloads"),
        )
        .unwrap();
        Env {
            dir,
            server: TestServer::start(),
            podcasts,
        }
    }

    /// Serves the main fixture feed and its audio, and subscribes to it.
    pub fn subscribed() -> (Env, Feed) {
        let mut env = Env::new();
        env.server.serve_fixture("/feed.xml", "feed.xml");
        env.server.serve_audio("/audio/ep1.mp3", 200_000);
        env.server.serve_audio("/audio/ep2.mp3", 150_000);
        env.server.serve_audio("/audio/ep3.mp3", 250_000);
        env.server.serve_audio("/audio/ep4.mp3", 100_000);
        let feed = env
            .podcasts
            .subscribe(&env.server.url("/feed.xml"))
            .unwrap();
        (env, feed)
    }

    pub fn download_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("downloads")
    }

    pub fn download(&mut self, episode_id: i64) -> std::path::PathBuf {
        self.podcasts
            .download(episode_id, &mut |_, _| {}, &AtomicBool::new(false))
            .unwrap()
    }
}
