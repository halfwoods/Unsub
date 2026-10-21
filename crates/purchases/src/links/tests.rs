//! The fixtures are trimmed from a real MusicBrainz search and release browse for "OK Computer".
//! The Bandcamp, 7digital, Amazon, Juno and iTunes entries in the releases were added by hand,
//! since the real album happens not to link to those stores.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tiny_http::{Header, Response, Server};

use super::*;
use crate::error::Error;

const SEARCH: &str = include_str!("../../tests/fixtures/musicbrainz-search.json");
const RELEASES: &str = include_str!("../../tests/fixtures/musicbrainz-releases.json");
const NO_HITS: &str =
    r#"{"created": "2026-10-19T00:00:00.000Z", "count": 0, "offset": 0, "release-groups": []}"#;
const DAY: i64 = 24 * 60 * 60;

#[derive(Clone)]
struct Recorded {
    url: String,
    user_agent: Option<String>,
}

/// A stand-in MusicBrainz on a local port that logs what it was asked.
struct TestServer {
    base_url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    server: Arc<Server>,
}

impl TestServer {
    /// `answer` gets the request path and query and returns a status and body.
    fn start(answer: impl Fn(&str) -> (u16, String) + Send + 'static) -> Self {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let port = server.server_addr().to_ip().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (worker, log) = (Arc::clone(&server), Arc::clone(&requests));
        thread::spawn(move || {
            for request in worker.incoming_requests() {
                let user_agent = request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv("User-Agent"))
                    .map(|header| header.value.to_string());
                let url = request.url().to_owned();
                let (status, body) = answer(&url);
                log.lock().unwrap().push(Recorded { url, user_agent });
                let json = Header::from_bytes("Content-Type", "application/json").unwrap();
                let _ = request.respond(
                    Response::from_string(body)
                        .with_status_code(status)
                        .with_header(json),
                );
            }
        });
        Self {
            base_url: format!("http://127.0.0.1:{port}/ws/2"),
            requests,
            server,
        }
    }

    /// The real fixtures: the search finds OK Computer, the browse lists its releases.
    fn album() -> Self {
        Self::start(|url| {
            if url.starts_with("/ws/2/release-group?") {
                (200, SEARCH.to_owned())
            } else if url.starts_with("/ws/2/release?") {
                (200, RELEASES.to_owned())
            } else {
                (404, String::new())
            }
        })
    }

    fn client(&self) -> MusicBrainz {
        MusicBrainz::new(&self.base_url, Duration::ZERO)
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

fn inbox() -> (tempfile::TempDir, Mutex<Inbox>) {
    let dir = tempfile::tempdir().unwrap();
    let inbox = Inbox::open(&dir.path().join("ledger.db"), &dir.path().join("library")).unwrap();
    (dir, Mutex::new(inbox))
}

fn link(store: &str, url: &str) -> StoreLink {
    StoreLink {
        store: store.to_owned(),
        url: url.to_owned(),
    }
}

/// Query strings arrive percent-encoded; tests compare against the plain text.
fn decoded(url: &str) -> String {
    let bytes = url.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes
            .get(index + 1..index + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                index += 3;
            }
            (byte, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn lists_every_purchase_for_download_link_once_with_its_store() {
    let server = TestServer::album();
    let (_dir, inbox) = inbox();

    let links =
        purchase_links_at(&server.client(), "Radiohead", "OK Computer", &inbox, 1_000).unwrap();

    assert_eq!(
        links,
        [
            link(
                "HDtracks",
                "https://www.hdtracks.com/#/album/5e043ed28e725b3854a2699e"
            ),
            link(
                "Qobuz",
                "https://www.qobuz.com/us-en/album/ok-computer-oknotok-1997-2017-radiohead/0634904086831"
            ),
            link(
                "oknotok17-us.wasteheadquarters.com",
                "https://oknotok17-us.wasteheadquarters.com/products/digital"
            ),
            link(
                "HDtracks",
                "http://www.hdtracks.com/ok-computer-oknotok-1997-2017"
            ),
            link("iTunes", "https://music.apple.com/us/album/1239489060"),
            link(
                "Bandcamp",
                "https://radiohead.bandcamp.com/album/ok-computer"
            ),
            link(
                "7digital",
                "https://www.7digital.com/artist/radiohead/release/ok-computer-oknotok-1997-2017"
            ),
            link("Amazon", "https://music.amazon.co.uk/albums/B01DPSNU78"),
            link(
                "iTunes",
                "https://itunes.apple.com/gb/album/ok-computer/1239489060"
            ),
            link(
                "junodownload.com",
                "https://www.junodownload.com/products/radiohead-ok-computer/123/"
            ),
        ]
    );
}

#[test]
fn identifies_itself_and_asks_for_json() {
    let server = TestServer::album();
    let (_dir, inbox) = inbox();

    purchase_links_at(&server.client(), "Radiohead", "OK Computer", &inbox, 1_000).unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let expected = format!("Unsub/{} ( local music player )", env!("CARGO_PKG_VERSION"));
    for request in &requests {
        assert_eq!(request.user_agent.as_deref(), Some(expected.as_str()));
        assert!(request.url.contains("fmt=json"), "{}", request.url);
    }
    let search = decoded(&requests[0].url);
    assert!(
        search.contains(r#"query=artist:"Radiohead" AND releasegroup:"OK Computer""#),
        "{search}"
    );
    let browse = decoded(&requests[1].url);
    assert!(
        browse.contains("release-group=b1392450-e666-3926-a536-22c65f834433"),
        "{browse}"
    );
    assert!(browse.contains("inc=url-rels"), "{browse}");
}

#[test]
fn quotes_in_names_are_escaped_in_the_search() {
    let server = TestServer::start(|_| (200, NO_HITS.to_owned()));
    let (_dir, inbox) = inbox();

    purchase_links_at(
        &server.client(),
        r#"Say "Hi""#,
        r"Back\slash",
        &inbox,
        1_000,
    )
    .unwrap();

    let search = decoded(&server.requests()[0].url);
    assert!(
        search.contains(r#"artist:"Say \"Hi\"" AND releasegroup:"Back\\slash""#),
        "{search}"
    );
}

#[test]
fn answers_are_cached_for_thirty_days() {
    let server = TestServer::album();
    let (_dir, inbox) = inbox();
    let client = server.client();
    let start = 5_000_000;

    let first = purchase_links_at(&client, "Radiohead", "OK Computer", &inbox, start).unwrap();
    assert_eq!(server.requests().len(), 2);

    // Different capitalization and spacing is the same album.
    let cached = purchase_links_at(
        &client,
        "radiohead",
        " OK  Computer ",
        &inbox,
        start + 29 * DAY,
    )
    .unwrap();
    assert_eq!(cached, first);
    assert_eq!(
        server.requests().len(),
        2,
        "within 30 days nothing is requested"
    );

    let refreshed = purchase_links_at(
        &client,
        "Radiohead",
        "OK Computer",
        &inbox,
        start + 31 * DAY,
    )
    .unwrap();
    assert_eq!(refreshed, first);
    assert_eq!(server.requests().len(), 4, "after 30 days it asks again");
}

#[test]
fn an_album_musicbrainz_does_not_know_is_cached_too() {
    let server = TestServer::start(|_| (200, NO_HITS.to_owned()));
    let (_dir, inbox) = inbox();
    let client = server.client();

    assert!(
        purchase_links_at(&client, "Nobody", "Nothing", &inbox, 1_000)
            .unwrap()
            .is_empty()
    );
    assert!(
        purchase_links_at(&client, "Nobody", "Nothing", &inbox, 2_000)
            .unwrap()
            .is_empty()
    );

    assert_eq!(server.requests().len(), 1);
}

#[test]
fn the_cache_is_not_locked_while_musicbrainz_is_asked() {
    let (_dir, inbox) = inbox();
    let inbox = Arc::new(inbox);
    let asked = Arc::clone(&inbox);
    let free_during_requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&free_during_requests);
    let server = TestServer::start(move |url| {
        seen.lock().unwrap().push(asked.try_lock().is_ok());
        if url.starts_with("/ws/2/release-group?") {
            (200, SEARCH.to_owned())
        } else {
            (200, RELEASES.to_owned())
        }
    });

    purchase_links_at(&server.client(), "Radiohead", "OK Computer", &inbox, 1_000).unwrap();

    assert_eq!(*free_during_requests.lock().unwrap(), [true, true]);
}

#[test]
fn a_hit_for_another_artist_or_a_weak_score_is_not_a_match() {
    let wrong_artist = SEARCH.replace(
        "\"name\": \"Radiohead\"",
        "\"name\": \"Vitamin String Quartet\"",
    );
    let weak_score = SEARCH.replace("\"score\": 100", "\"score\": 60");
    for body in [wrong_artist, weak_score] {
        let server = TestServer::start(move |_| (200, body.clone()));
        let (_dir, inbox) = inbox();

        let links =
            purchase_links_at(&server.client(), "Radiohead", "OK Computer", &inbox, 1_000).unwrap();

        assert!(links.is_empty());
        assert_eq!(
            server.requests().len(),
            1,
            "no release lookup for a non-match"
        );
    }
}

#[test]
fn a_release_group_with_no_download_links_gives_an_empty_list() {
    let server = TestServer::start(|url| {
        if url.starts_with("/ws/2/release-group?") {
            (200, SEARCH.to_owned())
        } else {
            (200, r#"{"release-count": 1, "releases": [{"id": "x", "relations": [{"type": "discogs", "url": {"resource": "https://www.discogs.com/release/1"}}]}]}"#.to_owned())
        }
    });
    let (_dir, inbox) = inbox();

    let links =
        purchase_links_at(&server.client(), "Radiohead", "OK Computer", &inbox, 1_000).unwrap();

    assert!(links.is_empty());
}

#[test]
fn a_failing_server_is_an_error_and_is_not_cached() {
    let server = TestServer::start(|_| (503, String::new()));
    let (_dir, inbox) = inbox();
    let client = server.client();

    assert!(matches!(
        purchase_links_at(&client, "Radiohead", "OK Computer", &inbox, 1_000),
        Err(Error::Http(_))
    ));
    assert!(purchase_links_at(&client, "Radiohead", "OK Computer", &inbox, 1_001).is_err());

    assert_eq!(
        server.requests().len(),
        2,
        "the failure was asked about again"
    );
}

#[test]
fn requests_are_spaced_by_the_minimum_interval() {
    let server = TestServer::album();
    let (_dir, inbox) = inbox();
    let interval = Duration::from_millis(400);
    let client = MusicBrainz::new(&server.base_url, interval);

    let started = Instant::now();
    purchase_links_at(&client, "Radiohead", "OK Computer", &inbox, 1_000).unwrap();

    // Two requests: the second had to wait out the interval after the first.
    assert!(started.elapsed() >= interval);
}

#[test]
fn titles_match_ignoring_case_punctuation_and_edition_notes() {
    assert!(same_title("OK Computer", "ok computer"));
    assert!(same_title("OK Computer (Deluxe Edition)", "OK Computer"));
    assert!(same_title("Kid A [Remastered]", "Kid A"));
    assert!(same_title("What's Going On?", "Whats Going On"));
    assert!(!same_title("Abbey Road", "Abbey Road Sessions"));
    assert!(
        !same_title("(Untitled)", "(Untitled)"),
        "nothing left to compare"
    );
}

#[test]
fn names_stores_by_host() {
    for (url, store) in [
        (
            "https://radiohead.bandcamp.com/album/ok-computer",
            "Bandcamp",
        ),
        ("https://www.qobuz.com/us-en/album/x/123", "Qobuz"),
        ("https://www.7digital.com/artist/x", "7digital"),
        ("http://hdtracks.com/#/album/1", "HDtracks"),
        ("https://music.apple.com/us/album/1", "iTunes"),
        ("https://itunes.apple.com/us/album/1", "iTunes"),
        ("https://www.amazon.com/dp/B000", "Amazon"),
        ("https://www.amazon.co.uk/gp/product/B000", "Amazon"),
        ("https://music.amazon.de/albums/B000", "Amazon"),
        (
            "https://www.junodownload.com/products/x",
            "junodownload.com",
        ),
        ("https://user@shop.example:8080/x?y=1", "shop.example"),
        // A name that merely contains a store's is not that store.
        ("https://notbandcamp.com/x", "notbandcamp.com"),
        ("https://bandcamp.com.evil.test/x", "bandcamp.com.evil.test"),
        ("https://notamazon.com/x", "notamazon.com"),
    ] {
        assert_eq!(store_name(url), store, "{url}");
    }
}
