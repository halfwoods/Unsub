//! HTTP for everything here, over two transports.
//!
//! `https://` goes through ureq. Plain `http://` goes through a small client of our own, because
//! old SHOUTcast servers answer `ICY 200 OK`, which is not an HTTP status line and which ureq (like
//! most HTTP libraries) rejects. The small client also gives exact control over buffering, which
//! matters when the body is an endless stream with metadata woven through it, and it can be told to
//! stop from another thread.

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use ureq::unversioned::{
    resolver::DefaultResolver,
    transport::{Connector, DefaultConnector},
};
use url::Url;

use crate::{
    error::{Error, Result},
    stall::StallConnector,
    text::decode,
};

const MAX_REDIRECTS: usize = 5;
/// The head of a response (status line and headers) is never this big unless something is wrong.
const MAX_HEAD_BYTES: usize = 64 * 1024;
/// How often a blocked read wakes to check for cancellation.
const TICK: Duration = Duration::from_millis(250);

/// Lets another thread stop a read that is waiting on the network. Plain `http://` streams stop
/// within a quarter of a second; `https://` ones when the stall timeout next fires.
#[derive(Debug, Clone, Default)]
pub struct CancelHandle(Arc<AtomicBool>);

impl CancelHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Waits out `duration`, giving up early with [`Error::Cancelled`] if cancelled meanwhile.
    pub(crate) fn sleep(&self, duration: Duration) -> Result<()> {
        let deadline = Instant::now() + duration;
        loop {
            if self.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            std::thread::sleep(left.min(Duration::from_millis(50)));
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub user_agent: String,
    pub connect_timeout: Duration,
    /// How long a read may wait for data. A stream that is merely long never trips it; one that goes
    /// silent does.
    pub stall_timeout: Duration,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            user_agent: concat!("AntiStreaming/", env!("CARGO_PKG_VERSION")).to_owned(),
            connect_timeout: Duration::from_secs(10),
            stall_timeout: Duration::from_secs(15),
        }
    }
}

/// Response headers, looked up without regard to case.
#[derive(Debug, Clone, Default)]
pub(crate) struct Headers(Vec<(String, String)>);

impl Headers {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The media type without parameters, lower-cased: `audio/mpeg` from `audio/mpeg; charset=x`.
    pub fn media_type(&self) -> Option<String> {
        self.get("content-type")
            .map(|value| {
                value
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
            })
            .filter(|value| !value.is_empty())
    }

    #[cfg(test)]
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Self(
            pairs
                .iter()
                .map(|&(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
        )
    }
}

pub(crate) fn parse_url(address: &str) -> Result<Url> {
    Url::parse(address.trim())
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        .ok_or_else(|| Error::InvalidUrl(address.to_owned()))
}

pub(crate) struct Response {
    pub status: u16,
    /// Where the redirects ended, against which relative addresses in the body resolve.
    pub url: Url,
    pub headers: Headers,
    pub body: Box<dyn Read + Send>,
}

#[derive(Clone)]
pub(crate) struct Http {
    config: HttpConfig,
    agent: ureq::Agent,
    cancel: CancelHandle,
}

impl Http {
    pub fn new(config: HttpConfig, cancel: CancelHandle) -> Self {
        let agent_config = ureq::Agent::config_builder()
            .user_agent(&config.user_agent)
            .timeout_connect(Some(config.connect_timeout))
            .timeout_recv_response(Some(config.stall_timeout))
            // Redirects are followed here, so that both transports share one hop budget.
            .max_redirects(0)
            .http_status_as_error(false)
            .build();
        let agent = ureq::Agent::with_parts(
            agent_config,
            DefaultConnector::new().chain(StallConnector::new(config.stall_timeout)),
            DefaultResolver::default(),
        );
        Self {
            config,
            agent,
            cancel,
        }
    }

    pub fn cancel_handle(&self) -> &CancelHandle {
        &self.cancel
    }

    /// GETs `url`, following up to five redirects. Anything but a 2xx in the end is an error.
    pub fn get(&self, url: &Url, headers: &[(&str, &str)]) -> Result<Response> {
        let mut url = url.clone();
        for _ in 0..=MAX_REDIRECTS {
            let response = match url.scheme() {
                "http" => self.raw_get(&url, headers)?,
                "https" => self.ureq_get(&url, headers)?,
                _ => return Err(Error::InvalidUrl(url.to_string())),
            };
            match response.status {
                301 | 302 | 303 | 307 | 308 => {
                    let location = response
                        .headers
                        .get("location")
                        .ok_or_else(|| Error::Http {
                            url: url.to_string(),
                            status: response.status,
                        })?;
                    url = url
                        .join(location)
                        .map_err(|_| Error::InvalidUrl(location.to_owned()))?;
                }
                200..=299 => return Ok(response),
                status => {
                    return Err(Error::Http {
                        url: url.to_string(),
                        status,
                    });
                }
            }
        }
        Err(Error::TooManyRedirects(url.to_string()))
    }

    /// A small document in full. More than `limit` bytes is an error, not a truncation.
    pub fn get_document(&self, url: &Url, limit: u64) -> Result<(Response, Vec<u8>)> {
        let mut response = self.get(url, &[])?;
        let mut body = Vec::new();
        (&mut response.body)
            .take(limit + 1)
            .read_to_end(&mut body)
            .map_err(|error| io_error(url, &error))?;
        if body.len() as u64 > limit {
            return Err(Error::Response {
                what: "document",
                detail: format!("{url} is larger than {limit} bytes"),
            });
        }
        Ok((response, body))
    }

    fn ureq_get(&self, url: &Url, headers: &[(&str, &str)]) -> Result<Response> {
        let mut request = self.agent.get(url.as_str());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.call().map_err(|error| Error::network(url, error))?;
        let status = response.status().as_u16();
        let headers = Headers(
            response
                .headers()
                .iter()
                .map(|(name, value)| (name.as_str().to_owned(), decode(value.as_bytes())))
                .collect(),
        );
        Ok(Response {
            status,
            url: url.clone(),
            headers,
            body: Box::new(response.into_body().into_reader()),
        })
    }

    fn raw_get(&self, url: &Url, headers: &[(&str, &str)]) -> Result<Response> {
        let host = url
            .host_str()
            .ok_or_else(|| Error::InvalidUrl(url.to_string()))?;
        let port = url.port_or_known_default().unwrap_or(80);
        let stream = self.connect(url, host, port)?;
        let mut request = format!("GET {} HTTP/1.1\r\n", path_and_query(url));
        request.push_str(&format!("Host: {}\r\n", host_header(url, host)));
        request.push_str(&format!(
            "User-Agent: {}\r\nAccept: */*\r\nConnection: close\r\n",
            self.config.user_agent
        ));
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        (&stream)
            .write_all(request.as_bytes())
            .map_err(|error| Error::network(url, error))?;

        let mut reader = BufReader::with_capacity(
            16 * 1024,
            Ticking::new(stream, self.config.stall_timeout, self.cancel.clone()),
        );
        let head = read_head(&mut reader).map_err(|error| match error {
            HeadError::Io(error) => io_error(url, &error),
            HeadError::Malformed(detail) => Error::network(url, detail),
        })?;
        let body = body_reader(reader, &head.headers);
        Ok(Response {
            status: head.status,
            url: url.clone(),
            headers: head.headers,
            body,
        })
    }

    fn connect(&self, url: &Url, host: &str, port: u16) -> Result<TcpStream> {
        let addresses = (host, port)
            .to_socket_addrs()
            .map_err(|error| Error::network(url, error))?;
        let mut last = None;
        for address in addresses {
            if self.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            match TcpStream::connect_timeout(&address, self.config.connect_timeout) {
                Ok(stream) => {
                    stream.set_nodelay(true).ok();
                    stream
                        .set_read_timeout(Some(TICK.min(self.config.stall_timeout)))
                        .map_err(|e| Error::network(url, e))?;
                    stream
                        .set_write_timeout(Some(self.config.connect_timeout))
                        .map_err(|e| Error::network(url, e))?;
                    return Ok(stream);
                }
                Err(error) => last = Some(error),
            }
        }
        Err(Error::network(
            url,
            last.map_or_else(|| "no address".to_owned(), |error| error.to_string()),
        ))
    }
}

/// An `io::Error` from reading `url`, as the crate's error.
pub(crate) fn io_error(url: &Url, error: &io::Error) -> Error {
    if crate::error::is_cancelled(error) {
        Error::Cancelled
    } else if error.kind() == io::ErrorKind::TimedOut {
        Error::Stalled
    } else {
        Error::network(url, error)
    }
}

fn path_and_query(url: &Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    }
}

fn host_header(url: &Url, host: &str) -> String {
    // IPv6 literals keep their brackets in a Host header.
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

/// A socket reader whose waits are bounded by the stall timeout and interruptible: the socket wakes
/// every few hundred milliseconds so that a cancel is noticed promptly.
struct Ticking {
    stream: TcpStream,
    stall: Duration,
    cancel: CancelHandle,
    last_data: Instant,
}

impl Ticking {
    fn new(stream: TcpStream, stall: Duration, cancel: CancelHandle) -> Self {
        Self {
            stream,
            stall,
            cancel,
            last_data: Instant::now(),
        }
    }
}

impl Read for Ticking {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.cancel.is_cancelled() {
                return Err(Error::Cancelled.into_io());
            }
            match self.stream.read(buf) {
                Ok(read) => {
                    self.last_data = Instant::now();
                    return Ok(read);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    if self.last_data.elapsed() >= self.stall {
                        return Err(Error::Stalled.into_io());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

struct Head {
    status: u16,
    headers: Headers,
}

enum HeadError {
    Io(io::Error),
    Malformed(String),
}

impl From<io::Error> for HeadError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Reads the status line and headers. The status line may be `HTTP/1.x NNN` or SHOUTcast's
/// `ICY NNN`; SHOUTcast also writes headers as `icy-name:value` with no space.
fn read_head(reader: &mut impl BufRead) -> std::result::Result<Head, HeadError> {
    let mut line = Vec::new();
    let mut total = 0;
    let mut status = None;
    let mut headers = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            return Err(HeadError::Malformed(
                "the connection closed before the response".to_owned(),
            ));
        }
        total += read;
        if total > MAX_HEAD_BYTES {
            return Err(HeadError::Malformed(
                "the response head is too long".to_owned(),
            ));
        }
        // Header text is not always UTF-8: SHOUTcast writes station names in Latin-1.
        let text = decode(&line);
        let text = text.trim_end_matches(['\r', '\n']);
        match status {
            None => {
                if text.is_empty() {
                    continue;
                }
                status =
                    Some(parse_status_line(text).ok_or_else(|| {
                        HeadError::Malformed(format!("bad status line {text:?}"))
                    })?);
            }
            Some(_) if text.is_empty() => break,
            Some(_) => {
                if let Some((name, value)) = text.split_once(':') {
                    headers.push((name.trim().to_owned(), value.trim().to_owned()));
                }
            }
        }
    }
    Ok(Head {
        status: status.unwrap_or(0),
        headers: Headers(headers),
    })
}

fn parse_status_line(line: &str) -> Option<u16> {
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !(version.starts_with("HTTP/") || version.eq_ignore_ascii_case("ICY")) {
        return None;
    }
    parts.next()?.parse().ok()
}

/// The body as the headers frame it: chunked, a given length, or until the connection closes.
fn body_reader(reader: BufReader<Ticking>, headers: &Headers) -> Box<dyn Read + Send> {
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        return Box::new(Chunked {
            inner: reader,
            left: 0,
            finished: false,
        });
    }
    match headers
        .get("content-length")
        .and_then(|value| value.parse::<u64>().ok())
    {
        Some(length) => Box::new(reader.take(length)),
        None => Box::new(reader),
    }
}

/// `Transfer-Encoding: chunked`.
struct Chunked<R> {
    inner: R,
    left: u64,
    finished: bool,
}

impl<R: BufRead> Chunked<R> {
    fn next_chunk(&mut self) -> io::Result<()> {
        let mut line = String::new();
        self.inner.read_line(&mut line)?;
        let size = line.trim().split(';').next().unwrap_or_default();
        let size = u64::from_str_radix(size, 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"))?;
        if size == 0 {
            // Trailers, up to the blank line.
            loop {
                line.clear();
                if self.inner.read_line(&mut line)? == 0 || line.trim().is_empty() {
                    break;
                }
            }
            self.finished = true;
        }
        self.left = size;
        Ok(())
    }
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.finished {
            return Ok(0);
        }
        if self.left == 0 {
            self.next_chunk()?;
            if self.finished {
                return Ok(0);
            }
        }
        let wanted = buf
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buf[..wanted])?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.left -= read as u64;
        if self.left == 0 {
            // The CRLF that ends the chunk.
            let mut end = [0u8; 2];
            self.inner.read_exact(&mut end)?;
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Write, thread, time::Duration};

    use super::*;
    use crate::testing::{Server, head, not_found, ok, redirect};

    fn client(stall: Duration) -> Http {
        let config = HttpConfig {
            stall_timeout: stall,
            connect_timeout: Duration::from_secs(2),
            ..HttpConfig::default()
        };
        Http::new(config, CancelHandle::new())
    }

    fn url(server: &Server, path: &str) -> Url {
        Url::parse(&server.url(path)).unwrap()
    }

    fn body_of(response: Response) -> Vec<u8> {
        let mut body = Vec::new();
        let mut reader = response.body;
        reader.read_to_end(&mut body).unwrap();
        body
    }

    #[test]
    fn shoutcast_status_line_and_unspaced_latin_1_headers_are_accepted() {
        let server = Server::start(|_, stream| {
            let _ = stream.write_all(b"ICY 200 OK\r\nicy-name:Caf\xe9 Radio\r\nicy-br:128\r\nContent-Type: audio/mpeg\r\n\r\naudio");
        });
        let response = client(Duration::from_secs(2))
            .get(&url(&server, "/"), &[])
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.headers.get("icy-name"), Some("Caf\u{e9} Radio"));
        assert_eq!(response.headers.get("ICY-BR"), Some("128"));
        assert_eq!(response.headers.media_type().as_deref(), Some("audio/mpeg"));
        assert_eq!(body_of(response), b"audio");
    }

    #[test]
    fn the_client_identifies_itself_and_sends_requested_headers() {
        let server = Server::start(|_, stream| ok(stream, "text/plain", b"hi"));
        client(Duration::from_secs(2))
            .get(&url(&server, "/x?y=1"), &[("Icy-MetaData", "1")])
            .unwrap();
        let request = &server.requests()[0];
        assert_eq!(request.path, "/x?y=1");
        assert_eq!(request.header("icy-metadata"), Some("1"));
        assert!(
            request
                .header("user-agent")
                .is_some_and(|agent| agent.starts_with("AntiStreaming/"))
        );
        assert_eq!(
            request
                .header("host")
                .map(|host| host.starts_with("127.0.0.1:")),
            Some(true)
        );
    }

    #[test]
    fn the_ureq_transport_behaves_the_same_for_plain_http() {
        let server = Server::start(|request, stream| match request.path.as_str() {
            "/moved" => redirect(stream, "/there"),
            _ => ok(stream, "audio/mpeg", b"data"),
        });
        let http = client(Duration::from_secs(2));
        let response = http
            .ureq_get(&url(&server, "/there"), &[("Icy-MetaData", "1")])
            .unwrap();
        assert_eq!(
            (response.status, response.headers.media_type().as_deref()),
            (200, Some("audio/mpeg"))
        );
        assert_eq!(body_of(response), b"data");
        assert_eq!(server.requests()[0].header("icy-metadata"), Some("1"));
        // Redirects are left to `get`, so both transports share one hop budget.
        assert_eq!(
            http.ureq_get(&url(&server, "/moved"), &[]).unwrap().status,
            302
        );
    }

    #[test]
    fn redirects_are_followed_relative_and_absolute() {
        let server = Server::start(|request, stream| match request.path.as_str() {
            "/a" => redirect(stream, "/b"),
            "/b" => redirect(stream, "c"),
            "/c" => ok(stream, "text/plain", b"arrived"),
            _ => not_found(stream),
        });
        let response = client(Duration::from_secs(2))
            .get(&url(&server, "/a"), &[])
            .unwrap();
        assert_eq!(response.url.path(), "/c");
        assert_eq!(body_of(response), b"arrived");
    }

    #[test]
    fn a_redirect_loop_stops_at_the_hop_cap() {
        let server = Server::start(|_, stream| redirect(stream, "/again"));
        let error = client(Duration::from_secs(2))
            .get(&url(&server, "/again"), &[])
            .err()
            .unwrap();
        assert!(matches!(error, Error::TooManyRedirects(_)), "{error}");
        assert_eq!(server.requests().len(), MAX_REDIRECTS + 1);
    }

    #[test]
    fn error_statuses_and_refused_connections_are_errors() {
        let server = Server::start(|_, stream| not_found(stream));
        let http = client(Duration::from_secs(2));
        assert!(matches!(
            http.get(&url(&server, "/"), &[]).err().unwrap(),
            Error::Http { status: 404, .. }
        ));
        let dead = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap()
        };
        assert!(matches!(
            http.get(&dead, &[]).err().unwrap(),
            Error::Network { .. }
        ));
    }

    #[test]
    fn chunked_and_close_delimited_and_length_delimited_bodies_are_decoded() {
        let server = Server::start(|request, stream| match request.path.as_str() {
            "/chunked" => {
                let _ = head(
                    stream,
                    "HTTP/1.1 200 OK",
                    &[("Transfer-Encoding", "chunked")],
                );
                let _ = stream
                    .write_all(b"5\r\nhello\r\n7;ext=1\r\n, world\r\n0\r\nX-Trailer: 1\r\n\r\n");
            }
            "/close" => {
                let _ = head(stream, "HTTP/1.0 200 OK", &[]);
                let _ = stream.write_all(b"until the end");
            }
            _ => {
                let _ = head(stream, "HTTP/1.1 200 OK", &[("Content-Length", "4")]);
                let _ = stream.write_all(b"only this is body");
            }
        });
        let http = client(Duration::from_secs(2));
        assert_eq!(
            body_of(http.get(&url(&server, "/chunked"), &[]).unwrap()),
            b"hello, world"
        );
        assert_eq!(
            body_of(http.get(&url(&server, "/close"), &[]).unwrap()),
            b"until the end"
        );
        assert_eq!(
            body_of(http.get(&url(&server, "/length"), &[]).unwrap()),
            b"only"
        );
    }

    #[test]
    fn documents_over_the_limit_are_refused_not_truncated() {
        let server = Server::start(|_, stream| ok(stream, "text/plain", &[b'x'; 100]));
        let http = client(Duration::from_secs(2));
        assert_eq!(
            http.get_document(&url(&server, "/"), 100).unwrap().1.len(),
            100
        );
        assert!(matches!(
            http.get_document(&url(&server, "/"), 99).err().unwrap(),
            Error::Response { .. }
        ));
    }

    #[test]
    fn a_server_that_goes_silent_stalls_the_read_instead_of_hanging() {
        let server = Server::start(|_, stream| {
            let _ = head(stream, "HTTP/1.1 200 OK", &[("Content-Type", "audio/mpeg")]);
            let _ = stream.write_all(b"some audio");
            thread::sleep(Duration::from_secs(3));
        });
        let mut response = client(Duration::from_millis(400))
            .get(&url(&server, "/"), &[])
            .unwrap();
        let mut buf = [0u8; 64];
        assert_eq!(response.body.read(&mut buf).unwrap(), 10);
        let started = Instant::now();
        let error = response.body.read(&mut buf).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_server_that_never_answers_is_a_stall_too() {
        let server = Server::start(|_, _| thread::sleep(Duration::from_secs(3)));
        let started = Instant::now();
        let error = client(Duration::from_millis(400))
            .get(&url(&server, "/"), &[])
            .err()
            .unwrap();
        assert!(matches!(error, Error::Stalled), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancelling_interrupts_a_read_that_is_waiting() {
        let server = Server::start(|_, stream| {
            let _ = head(stream, "HTTP/1.1 200 OK", &[]);
            thread::sleep(Duration::from_secs(5));
        });
        let http = client(Duration::from_secs(30));
        let cancel = http.cancel_handle().clone();
        let mut response = http.get(&url(&server, "/"), &[]).unwrap();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            cancel.cancel();
        });
        let started = Instant::now();
        let error = response.body.read(&mut [0u8; 8]).unwrap_err();
        assert!(crate::error::is_cancelled(&error), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn sleeping_stops_when_cancelled() {
        let cancel = CancelHandle::new();
        let other = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            other.cancel();
        });
        let started = Instant::now();
        assert!(matches!(
            cancel.sleep(Duration::from_secs(30)),
            Err(Error::Cancelled)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(CancelHandle::new().sleep(Duration::from_millis(10)).is_ok());
    }

    #[test]
    fn only_http_and_https_addresses_are_accepted() {
        assert!(parse_url(" http://radio.example.com:8000/live ").is_ok());
        assert!(parse_url("https://radio.example.com/").is_ok());
        for bad in [
            "",
            "radio.example.com",
            "ftp://radio.example.com/",
            "file:///etc/passwd",
            "mms://x/y",
            "http://",
        ] {
            assert!(
                matches!(parse_url(bad), Err(Error::InvalidUrl(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn media_types_are_normalised() {
        let headers = Headers::from_pairs(&[("Content-Type", "Audio/MPEG; charset=utf-8")]);
        assert_eq!(headers.media_type().as_deref(), Some("audio/mpeg"));
        assert_eq!(
            Headers::from_pairs(&[("Content-Type", " ")]).media_type(),
            None
        );
        assert_eq!(Headers::default().media_type(), None);
    }
}
