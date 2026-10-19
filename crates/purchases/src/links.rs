use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::error::Result;
use crate::ledger::{Inbox, unix_seconds};
use crate::model::StoreLink;

const BASE_URL: &str = "https://musicbrainz.org/ws/2";
/// MusicBrainz allows an average of one request per second per client.
const MIN_INTERVAL: Duration = Duration::from_secs(1);
const USER_AGENT: &str = concat!(
    "AntiStreaming/",
    env!("CARGO_PKG_VERSION"),
    " ( local music player )"
);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A release group with 40 releases comes back at around 1 MB.
const RESPONSE_LIMIT: u64 = 32 * 1024 * 1024;
const CACHE_AGE_SECONDS: i64 = 30 * 24 * 60 * 60;
/// Search hits scoring below this are different albums that merely share some words.
const MIN_SCORE: u32 = 90;
const PURCHASE_RELATION: &str = "purchase for download";

/// Where to buy an album for download, according to MusicBrainz. At most two requests are made,
/// and the answer, including an empty one, is cached in `cache` for 30 days.
pub fn purchase_links(artist: &str, album: &str, cache: &Inbox) -> Result<Vec<StoreLink>> {
    static CLIENT: OnceLock<MusicBrainz> = OnceLock::new();
    let client = CLIENT.get_or_init(|| MusicBrainz::new(BASE_URL, MIN_INTERVAL));
    purchase_links_at(client, artist, album, cache, unix_seconds())
}

pub(crate) fn purchase_links_at(
    client: &MusicBrainz,
    artist: &str,
    album: &str,
    cache: &Inbox,
    now: i64,
) -> Result<Vec<StoreLink>> {
    let key = cache_key(artist, album);
    if let Some(links) = cache.cached_links(&key, now - CACHE_AGE_SECONDS)? {
        return Ok(links);
    }
    let links = match client.find_release_group(artist, album)? {
        Some(release_group) => client.purchase_links_of(&release_group)?,
        None => Vec::new(),
    };
    cache.cache_links(&key, &links, now)?;
    Ok(links)
}

/// A MusicBrainz web service, spaced out to its rate limit. The base URL is a parameter so tests
/// can point it at a local server.
pub(crate) struct MusicBrainz {
    base_url: String,
    min_interval: Duration,
    agent: ureq::Agent,
    last_request: Mutex<Option<Instant>>,
}

impl MusicBrainz {
    pub(crate) fn new(base_url: &str, min_interval: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build();
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            min_interval,
            agent: ureq::Agent::new_with_config(config),
            last_request: Mutex::new(None),
        }
    }

    /// The id of the release group matching this artist and album, if MusicBrainz has one.
    fn find_release_group(&self, artist: &str, album: &str) -> Result<Option<String>> {
        let query = format!(
            "artist:\"{}\" AND releasegroup:\"{}\"",
            escape_phrase(artist),
            escape_phrase(album)
        );
        let body = self.get("release-group", &[("query", &query), ("limit", "5")])?;
        let hits: SearchResponse = serde_json::from_str(&body)?;
        Ok(hits
            .release_groups
            .into_iter()
            .find(|hit| {
                hit.score >= MIN_SCORE
                    && same_title(&hit.title, album)
                    && same_title(&hit.credited_artist(), artist)
            })
            .map(|hit| hit.id))
    }

    /// One browse request returns every release of the group with its URL relations, which is
    /// where the purchase links live.
    fn purchase_links_of(&self, release_group: &str) -> Result<Vec<StoreLink>> {
        let body = self.get(
            "release",
            &[
                ("release-group", release_group),
                ("inc", "url-rels"),
                ("limit", "100"),
            ],
        )?;
        let releases: BrowseResponse = serde_json::from_str(&body)?;
        let mut seen = HashSet::new();
        let mut links = Vec::new();
        for relation in releases
            .releases
            .iter()
            .flat_map(|release| &release.relations)
        {
            if relation.kind != PURCHASE_RELATION {
                continue;
            }
            let Some(url) = relation.url.as_ref().map(|url| url.resource.as_str()) else {
                continue;
            };
            if seen.insert(url) {
                links.push(StoreLink {
                    store: store_name(url),
                    url: url.to_owned(),
                });
            }
        }
        Ok(links)
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<String> {
        self.wait_for_turn();
        let mut request = self
            .agent
            .get(format!("{}/{path}", self.base_url))
            .query("fmt", "json");
        for (name, value) in query {
            request = request.query(*name, *value);
        }
        let mut response = request.call()?;
        Ok(response
            .body_mut()
            .with_config()
            .limit(RESPONSE_LIMIT)
            .read_to_string()?)
    }

    /// Sleeps until the minimum interval since the previous request has passed. The lock is held
    /// while sleeping so that concurrent callers queue up instead of all firing at once.
    fn wait_for_turn(&self) {
        let mut last = self
            .last_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = *last {
            let elapsed = previous.elapsed();
            if elapsed < self.min_interval {
                thread::sleep(self.min_interval - elapsed);
            }
        }
        *last = Some(Instant::now());
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(rename = "release-groups", default)]
    release_groups: Vec<GroupHit>,
}

#[derive(Deserialize)]
struct GroupHit {
    id: String,
    #[serde(default)]
    score: u32,
    #[serde(default)]
    title: String,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<Credit>,
}

#[derive(Deserialize)]
struct Credit {
    #[serde(default)]
    name: String,
    #[serde(default)]
    joinphrase: String,
}

impl GroupHit {
    /// "Simon" + " & " + "Garfunkel", the way the credit is printed on the album.
    fn credited_artist(&self) -> String {
        self.artist_credit
            .iter()
            .map(|credit| format!("{}{}", credit.name, credit.joinphrase))
            .collect()
    }
}

#[derive(Deserialize)]
struct BrowseResponse {
    #[serde(default)]
    releases: Vec<Release>,
}

#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    relations: Vec<Relation>,
}

#[derive(Deserialize)]
struct Relation {
    #[serde(rename = "type", default)]
    kind: String,
    url: Option<UrlResource>,
}

#[derive(Deserialize)]
struct UrlResource {
    resource: String,
}

/// Quotes inside a Lucene phrase are the only characters that need escaping.
fn escape_phrase(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Titles match when they read the same ignoring case, punctuation and edition notes such as
/// "(Deluxe Edition)" or "[Remastered]".
fn same_title(a: &str, b: &str) -> bool {
    let (a, b) = (comparable(a), comparable(b));
    !a.is_empty() && a == b
}

fn comparable(title: &str) -> String {
    let mut depth = 0usize;
    let mut kept = String::new();
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            c if depth == 0 && c.is_alphanumeric() => kept.extend(c.to_lowercase()),
            _ => {}
        }
    }
    kept
}

fn cache_key(artist: &str, album: &str) -> String {
    let normalize = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    format!("{}\u{1f}{}", normalize(artist), normalize(album))
}

/// The store a purchase link points at, by its host.
fn store_name(url: &str) -> String {
    let host = host_of(url);
    let on = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if on("bandcamp.com") {
        "Bandcamp".to_owned()
    } else if on("qobuz.com") {
        "Qobuz".to_owned()
    } else if on("7digital.com") {
        "7digital".to_owned()
    } else if on("hdtracks.com") {
        "HDtracks".to_owned()
    } else if on("music.apple.com") || on("itunes.apple.com") {
        "iTunes".to_owned()
    } else if host.split('.').any(|label| label == "amazon") {
        "Amazon".to_owned()
    } else {
        // A store this crate has no name for is still a real place to buy.
        host.strip_prefix("www.").unwrap_or(&host).to_owned()
    }
}

fn host_of(url: &str) -> String {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    host.split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests;
