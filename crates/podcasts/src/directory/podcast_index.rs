//! Podcast Index (<https://podcastindex.org>), the open podcast directory. Its API wants a key and
//! secret, which a build is given at compile time; see [`super::default_directory`].

use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;
use sha1::{Digest, Sha1};

use super::{
    CATEGORIES, Directory, Listing, Listings, RESULT_LIMIT, Source, dedupe, image_url,
    system_language,
};
use crate::error::{Error, Result};
use crate::http::{self, Budget};

const API: &str = "https://api.podcastindex.org/api/1.0";

pub(super) struct PodcastIndex {
    agent: ureq::Agent,
    key: String,
    secret: String,
    api: String,
    budget: Budget,
}

impl PodcastIndex {
    pub(super) fn new(key: &str, secret: &str) -> Self {
        Self {
            agent: http::agent(http::DEFAULT_STALL_TIMEOUT),
            key: key.to_owned(),
            secret: secret.to_owned(),
            api: API.to_owned(),
            // It is always asked first and Apple answers if it can't, so waiting long for it only
            // makes the fallback late.
            budget: Budget::QUICK,
        }
    }

    fn get(&self, endpoint: &str, query: &[(&str, &str)]) -> Result<Vec<u8>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let signed = auth_headers(&self.key, &self.secret, now);
        let headers: Vec<_> = signed
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        http::fetch_directory(
            &self.agent,
            &format!("{}/{endpoint}", self.api),
            query,
            &headers,
            self.budget,
        )
    }
}

/// The headers that sign a request: the key, the time, and a SHA-1 of the key, secret and time
/// together, in lower-case hex.
fn auth_headers(key: &str, secret: &str, unix_seconds: u64) -> [(&'static str, String); 3] {
    let date = unix_seconds.to_string();
    let digest = Sha1::digest(format!("{key}{secret}{date}").as_bytes());
    let authorization = digest.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    [
        ("X-Auth-Key", key.to_owned()),
        ("X-Auth-Date", date),
        ("Authorization", authorization),
    ]
}

impl Directory for PodcastIndex {
    fn search(&self, term: &str) -> Result<Listings> {
        let term = term.trim();
        if term.is_empty() {
            return Ok(listings(Vec::new()));
        }
        parse_feeds(&self.get("search/byterm", &[("q", term), ("max", RESULT_LIMIT)])?)
            .map(listings)
    }

    fn top(&self, category: Option<u32>) -> Result<Listings> {
        let mut query = vec![("max", RESULT_LIMIT)];
        let language = system_language();
        if let Some(language) = &language {
            query.push(("lang", language));
        }
        if let Some(id) = category {
            let category = CATEGORIES
                .iter()
                .find(|category| category.id == id)
                .ok_or_else(|| Error::Directory(format!("unknown category {id}")))?;
            query.push(("cat", category.podcast_index));
        }
        parse_feeds(&self.get("podcasts/trending", &query)?).map(listings)
    }

    fn source(&self) -> Source {
        Source::PodcastIndex
    }
}

fn listings(shows: Vec<Listing>) -> Listings {
    Listings {
        source: Source::PodcastIndex,
        shows,
    }
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    feeds: Vec<Feed>,
}

/// One show in a search or trending reply; trending lacks some of the fields.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Feed {
    url: Option<String>,
    title: Option<String>,
    author: Option<String>,
    owner_name: Option<String>,
    image: Option<String>,
    artwork: Option<String>,
    /// An object of names by id, or null.
    categories: Value,
    episode_count: Option<u32>,
    /// Non-zero for a feed that has stopped being reachable.
    dead: Option<i64>,
}

impl Feed {
    fn into_listing(self) -> Option<Listing> {
        let feed_url = self.url.filter(|url| !url.trim().is_empty())?;
        if self.dead.unwrap_or(0) != 0 {
            return None;
        }
        let present = |text: Option<String>| text.filter(|text| !text.is_empty());
        Some(Listing {
            title: present(self.title).unwrap_or_else(|| feed_url.clone()),
            feed_url,
            author: present(self.author)
                .or_else(|| present(self.owner_name))
                .unwrap_or_default(),
            // Its own copy of the best artwork it found comes before the feed's declared image.
            artwork_url: image_url(self.artwork).or_else(|| image_url(self.image)),
            genre: self
                .categories
                .as_object()
                .and_then(|names| names.values().find_map(Value::as_str))
                .unwrap_or_default()
                .to_owned(),
            episode_count: self.episode_count,
        })
    }
}

fn parse_feeds(body: &[u8]) -> Result<Vec<Listing>> {
    let reply: Reply =
        serde_json::from_slice(body).map_err(|err| Error::Directory(err.to_string()))?;
    let listings = reply
        .feeds
        .into_iter()
        .filter_map(Feed::into_listing)
        .collect();
    Ok(dedupe(listings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_that_never_answers_fails_within_its_budget() {
        // The connection is accepted by the operating system and nothing ever comes back, as
        // from a service that hangs.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut directory = PodcastIndex::new("key", "secret");
        directory.api = format!("http://{}", listener.local_addr().unwrap());
        directory.budget = Budget {
            connect: std::time::Duration::from_millis(200),
            total: std::time::Duration::from_millis(400),
        };

        let started = std::time::Instant::now();
        assert!(directory.search("anything").is_err());

        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn requests_are_signed_with_a_sha1_of_key_secret_and_time() {
        let headers = auth_headers("ABCDEFG", "secret", 1_700_000_000);
        assert_eq!(
            headers,
            [
                ("X-Auth-Key", "ABCDEFG".to_owned()),
                ("X-Auth-Date", "1700000000".to_owned()),
                (
                    "Authorization",
                    "137fd55e7fc5896ccd7e181663ab38baae9a4264".to_owned()
                ),
            ]
        );
        // A later second is a different signature.
        assert_eq!(
            auth_headers("ABCDEFG", "secret", 1_700_000_001)[2].1,
            "090c19aeeef47782140a98390aa2dd037e06fc0f"
        );
    }

    const SEARCH: &str = r#"{"status": "true", "count": 6, "description": "Found matching feeds.", "feeds": [
        {"id": 1, "title": "Show One", "url": "https://one.example/rss", "originalUrl": "http://one.example/old",
         "author": "Ann", "ownerName": "Ann Owner", "image": "https://img.example/one.jpg",
         "artwork": "https://img.example/one-art.jpg", "categories": {"104": "Tech News", "102": "Technology"},
         "episodeCount": 88, "language": "en", "dead": 0, "itunesId": 5, "podcastGuid": "abc"},
        {"id": 2, "title": "Gone", "url": "https://gone.example/rss", "dead": 1},
        {"id": 3, "title": "Image Only", "url": "https://two.example/rss", "ownerName": "Bea Owner",
         "image": "http://img.example/two.jpg", "artwork": "", "categories": null, "episodeCount": 3},
        {"id": 4, "title": "No Address", "url": ""},
        {"id": 5, "title": "Show One Again", "url": "https://one.example/rss"},
        {"id": 6, "url": "https://untitled.example/rss", "image": "javascript:alert(1)", "categories": []}
    ]}"#;

    #[test]
    fn search_keeps_the_fields_the_ui_shows() {
        let shows = parse_feeds(SEARCH.as_bytes()).unwrap();
        assert_eq!(
            shows[0],
            Listing {
                feed_url: "https://one.example/rss".into(),
                title: "Show One".into(),
                author: "Ann".into(),
                artwork_url: Some("https://img.example/one-art.jpg".into()),
                genre: "Technology".into(),
                episode_count: Some(88),
            }
        );
    }

    #[test]
    fn dead_unaddressed_and_repeated_feeds_are_skipped() {
        let shows = parse_feeds(SEARCH.as_bytes()).unwrap();
        let titles: Vec<_> = shows.iter().map(|show| show.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Show One", "Image Only", "https://untitled.example/rss"]
        );
    }

    #[test]
    fn missing_fields_are_filled_from_the_next_best() {
        let shows = parse_feeds(SEARCH.as_bytes()).unwrap();
        assert_eq!(shows[1].author, "Bea Owner");
        assert_eq!(
            shows[1].artwork_url.as_deref(),
            Some("http://img.example/two.jpg")
        );
        assert_eq!(shows[1].genre, "");
        assert_eq!(shows[2].artwork_url, None);
        assert_eq!(shows[2].episode_count, None);
    }

    #[test]
    fn trending_replies_are_read_the_same_way() {
        let trending = r#"{"status": "true", "count": 2, "max": 2, "description": "Found 2 feeds", "feeds": [
            {"id": 10, "url": "https://hot.example/rss", "title": "Hot", "author": "Cy",
             "image": "https://img.example/hot.jpg", "artwork": "https://img.example/hot-art.jpg",
             "trendScore": 99, "language": "en", "categories": {"59": "Politics"}},
            {"id": 11, "url": "https://warm.example/rss", "title": "Warm", "author": "Di",
             "image": "https://img.example/warm.jpg", "trendScore": 80, "language": "en", "categories": {}}
        ]}"#;
        let shows = parse_feeds(trending.as_bytes()).unwrap();
        assert_eq!(shows.len(), 2);
        assert_eq!(shows[0].genre, "Politics");
        assert_eq!(
            shows[1].artwork_url.as_deref(),
            Some("https://img.example/warm.jpg")
        );
        assert_eq!(shows[1].genre, "");
    }

    #[test]
    fn replies_without_feeds_are_empty_and_garbage_is_an_error() {
        assert!(
            parse_feeds(br#"{"status": "true", "count": 0, "feeds": []}"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_feeds(br#"{"status": "false", "description": "x"}"#)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(parse_feeds(b"<html>"), Err(Error::Directory(_))));
    }
}
