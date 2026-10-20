//! A fake radio server for tests: a bare `TcpListener`, so a test writes exactly the bytes a real
//! server would, including SHOUTcast's `ICY 200 OK`, and can drop the connection or go silent at
//! any point.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Debug, Clone)]
pub struct Request {
    /// Path and query, as sent.
    pub path: String,
    pub headers: Vec<(String, String)>,
    /// How many requests for this path there have been, this one included.
    pub nth: usize,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    log: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    /// Starts serving; `handler` writes the whole response, then returns, which closes the
    /// connection.
    pub fn start(handler: impl Fn(&Request, &mut TcpStream) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let stop = Arc::new(AtomicBool::new(false));
        let log: Arc<Mutex<Vec<Request>>> = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);
        {
            let (stop, log) = (stop.clone(), log.clone());
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(mut stream) = stream else { continue };
                    let (handler, log) = (handler.clone(), log.clone());
                    thread::spawn(move || {
                        let Some(mut request) = read_request(&mut stream) else {
                            return;
                        };
                        {
                            let mut log = log.lock().unwrap();
                            request.nth =
                                log.iter().filter(|seen| seen.path == request.path).count() + 1;
                            log.push(request.clone());
                        }
                        handler(&request, &mut stream);
                    });
                }
            });
        }
        Self { address, stop, log }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    /// The base address, for [`Directory::with_mirrors`](crate::Directory::with_mirrors).
    pub fn base(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap().clone()
    }

    pub fn hits(&self, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.path == path)
            .count()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop so it can see the flag.
        let _ = TcpStream::connect(self.address);
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.lines();
    let path = lines.next()?.split_whitespace().nth(1)?.to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    Some(Request {
        path,
        headers,
        nth: 0,
    })
}

/// Writes a status line and headers and the blank line after them.
pub fn head(
    stream: &mut TcpStream,
    status_line: &str,
    headers: &[(&str, &str)],
) -> std::io::Result<()> {
    let mut text = format!("{status_line}\r\n");
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("\r\n");
    stream.write_all(text.as_bytes())
}

/// A whole response with a `Content-Length`.
pub fn respond(stream: &mut TcpStream, status_line: &str, content_type: &str, body: &[u8]) {
    let length = body.len().to_string();
    let _ = head(
        stream,
        status_line,
        &[
            ("Content-Type", content_type),
            ("Content-Length", &length),
            ("Connection", "close"),
        ],
    );
    let _ = stream.write_all(body);
}

pub fn ok(stream: &mut TcpStream, content_type: &str, body: &[u8]) {
    respond(stream, "HTTP/1.1 200 OK", content_type, body);
}

pub fn redirect(stream: &mut TcpStream, location: &str) {
    let _ = head(
        stream,
        "HTTP/1.1 302 Found",
        &[
            ("Location", location),
            ("Content-Length", "0"),
            ("Connection", "close"),
        ],
    );
}

pub fn not_found(stream: &mut TcpStream) {
    respond(stream, "HTTP/1.1 404 Not Found", "text/plain", b"nope");
}
