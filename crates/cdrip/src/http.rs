//! The network, behind a trait so tests never touch it.

use std::{
    io::Read,
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::{Error, Result};

/// Fetches the public web resources a rip needs: MusicBrainz lookups, Cover Art Archive images and
/// the AccurateRip database.
pub trait MetadataClient: Send + Sync {
    /// GETs `url`, following redirects. HTTP 404 is `Ok(None)`: a disc, cover or checksum entry that
    /// does not exist is an answer, not a failure.
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>>;
}

/// MusicBrainz asks clients to average at most one request per second.
const MUSICBRAINZ_INTERVAL: Duration = Duration::from_secs(1);
/// Cover art is the largest thing fetched; this is far beyond any real album cover.
const MAX_BODY_BYTES: u64 = 32 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

/// The real thing: HTTPS over rustls, identifying itself the way MusicBrainz requires and keeping
/// to its rate limit.
pub struct UreqClient {
    agent: ureq::Agent,
    musicbrainz: RateLimiter,
}

impl UreqClient {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(user_agent())
            .timeout_global(Some(TIMEOUT))
            .build()
            .into();
        Self {
            agent,
            musicbrainz: RateLimiter::new(MUSICBRAINZ_INTERVAL),
        }
    }
}

impl Default for UreqClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MetadataClient for UreqClient {
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        let _turn = is_musicbrainz(url).then(|| self.musicbrainz.wait());
        let mut response = match self.agent.get(url).call() {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(404)) => return Ok(None),
            Err(err) => return Err(Error::Network(format!("{url}: {err}"))),
        };
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_BODY_BYTES)
            .read_to_end(&mut body)
            .map_err(|err| Error::Network(format!("{url}: {err}")))?;
        Ok(Some(body))
    }
}

/// `AntiStreaming/<version> ( local music player )`, the shape MusicBrainz asks for.
pub(crate) fn user_agent() -> String {
    format!(
        "AntiStreaming/{} ( local music player )",
        env!("CARGO_PKG_VERSION")
    )
}

fn is_musicbrainz(url: &str) -> bool {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .is_some_and(|rest| {
            let host = rest.split(['/', '?', ':']).next().unwrap_or_default();
            host == "musicbrainz.org" || host.ends_with(".musicbrainz.org")
        })
}

/// Spaces calls at least `interval` apart across threads.
struct RateLimiter {
    interval: Duration,
    last: Mutex<Option<Instant>>,
}

/// Holds the limiter's lock, so the next caller waits until this request has finished and then a
/// further interval after it.
struct Turn<'a>(std::sync::MutexGuard<'a, Option<Instant>>);

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        *self.0 = Some(Instant::now());
    }
}

impl RateLimiter {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Mutex::new(None),
        }
    }

    /// Blocks until a request may start; the returned guard marks the request's end when dropped.
    fn wait(&self) -> Turn<'_> {
        // A poisoned lock only means another request panicked; the timestamp is still usable.
        let last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(finished) = *last
            && let Some(remaining) = self.interval.checked_sub(finished.elapsed())
        {
            std::thread::sleep(remaining);
        }
        Turn(last)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        net::{SocketAddr, TcpListener},
        thread,
    };

    use super::*;

    #[test]
    fn user_agent_names_the_app_and_version() {
        assert_eq!(
            user_agent(),
            format!(
                "AntiStreaming/{} ( local music player )",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn musicbrainz_hosts_are_recognised_exactly() {
        assert!(is_musicbrainz("https://musicbrainz.org/ws/2/discid/x"));
        assert!(is_musicbrainz("http://beta.musicbrainz.org:8000/ws"));
        assert!(!is_musicbrainz(
            "https://coverartarchive.org/release/x/front"
        ));
        assert!(!is_musicbrainz("https://musicbrainz.org.example.com/"));
        assert!(!is_musicbrainz("https://notmusicbrainz.org/"));
        assert_eq!(MUSICBRAINZ_INTERVAL, Duration::from_secs(1));
    }

    #[test]
    fn limiter_spaces_requests_by_the_interval() {
        let limiter = RateLimiter::new(Duration::from_millis(60));
        let started = Instant::now();
        for _ in 0..3 {
            drop(limiter.wait());
        }
        // The first call is free; the next two each wait out the interval.
        assert!(
            started.elapsed() >= Duration::from_millis(120),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn limiter_counts_from_the_end_of_the_previous_request() {
        let limiter = RateLimiter::new(Duration::from_millis(80));
        let turn = limiter.wait();
        thread::sleep(Duration::from_millis(120));
        drop(turn);
        let started = Instant::now();
        drop(limiter.wait());
        assert!(started.elapsed() >= Duration::from_millis(60));
    }

    /// Serves canned responses to successive requests on localhost and records each request's
    /// header lines. Nothing leaves the machine.
    fn serve(responses: Vec<String>) -> (SocketAddr, thread::JoinHandle<Vec<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut seen = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    headers.push(line.trim().to_owned());
                }
                seen.push(headers);
                stream.write_all(response.as_bytes()).unwrap();
            }
            seen
        });
        (addr, handle)
    }

    #[test]
    fn get_sends_the_user_agent_follows_redirects_and_maps_404_to_none() {
        let ok = |body: &str| {
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        };
        let (addr, server) = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
            ok("payload"),
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
            "HTTP/1.1 500 Oops\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
        ]);
        let client = UreqClient::new();
        assert_eq!(
            client
                .get(&format!("http://{addr}/start"))
                .unwrap()
                .unwrap(),
            b"payload"
        );
        assert_eq!(client.get(&format!("http://{addr}/missing")).unwrap(), None);
        assert!(matches!(
            client.get(&format!("http://{addr}/broken")),
            Err(Error::Network(_))
        ));

        let seen = server.join().unwrap();
        let expected = format!("user-agent: {}", user_agent());
        for request in &seen {
            assert!(
                request
                    .iter()
                    .any(|line| line.eq_ignore_ascii_case(&expected)),
                "{request:?}"
            );
        }
        assert!(seen[1][0].starts_with("GET /final "));
    }
}
