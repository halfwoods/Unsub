//! The Radio Browser directory (<https://api.radio-browser.info>): community-run, no key needed.
//!
//! There is no single server. The project publishes a list of mirrors at
//! `all.api.radio-browser.info/json/servers`; requests go to one, in a random order so load
//! spreads, and move on to the next if it does not answer. Its terms ask for a descriptive
//! User-Agent, which [`HttpConfig`] provides by default.

mod genres;

use std::{
    fs,
    path::PathBuf,
    sync::Mutex,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use url::{Url, form_urlencoded};

pub use genres::{GENRES, Genre, genre, genres_of};

use crate::{
    error::{Error, Result},
    http::{CancelHandle, Http, HttpConfig},
};

/// Where the mirror list is published.
const DISCOVERY_URL: &str = "https://all.api.radio-browser.info";
/// Used if even the discovery address does not answer.
const FALLBACK_MIRRORS: [&str; 3] = [
    "https://de1.api.radio-browser.info",
    "https://de2.api.radio-browser.info",
    "https://fi1.api.radio-browser.info",
];
const TAG_LIMIT: u32 = 500;
/// Largest page asked of the server.
const MAX_PAGE: u32 = 500;
const MAX_RESPONSE_BYTES: u64 = 32 << 20;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// A station in the directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    pub uuid: String,
    pub name: String,
    /// The address to open: the directory's checked address when it has one, else the submitted one.
    pub url: String,
    pub homepage: String,
    pub favicon: String,
    /// Lower case, as submitted.
    pub tags: Vec<String>,
    pub country: String,
    /// `MP3`, `AAC`, `AAC+`, `OGG`, ... as the directory records it; may be `UNKNOWN`.
    pub codec: String,
    /// Zero when the directory does not know.
    pub bitrate_kbps: u32,
    /// Whether the address is an HLS playlist rather than a plain stream.
    pub is_hls: bool,
    pub votes: u64,
}

/// A tag and how many stations carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub name: String,
    pub station_count: u32,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawStation {
    stationuuid: String,
    name: String,
    url: String,
    url_resolved: String,
    homepage: String,
    favicon: String,
    tags: String,
    country: String,
    codec: String,
    bitrate: u32,
    hls: u8,
    votes: u64,
}

impl RawStation {
    /// `None` for an entry with nothing to play.
    fn into_station(self) -> Option<Station> {
        let url = [self.url_resolved, self.url]
            .into_iter()
            .map(|url| url.trim().to_owned())
            .find(|url| !url.is_empty())?;
        Some(Station {
            uuid: self.stationuuid,
            name: self.name.trim().to_owned(),
            url,
            homepage: self.homepage.trim().to_owned(),
            favicon: self.favicon.trim().to_owned(),
            tags: self
                .tags
                .split(',')
                .map(|tag| tag.trim().to_lowercase())
                .filter(|tag| !tag.is_empty())
                .collect(),
            country: self.country.trim().to_owned(),
            codec: self.codec.trim().to_owned(),
            bitrate_kbps: self.bitrate,
            is_hls: self.hls == 1,
            votes: self.votes,
        })
    }
}

#[derive(Deserialize)]
struct RawTag {
    name: String,
    #[serde(default)]
    stationcount: u32,
}

#[derive(Deserialize)]
struct RawServer {
    name: String,
}

#[derive(Deserialize)]
struct RawClick {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    message: String,
}

#[derive(Serialize, Deserialize)]
struct TagsCache {
    fetched_at: u64,
    tags: Vec<Tag>,
}

struct Mirrors {
    /// Base addresses in the order to try them. Empty until discovered.
    bases: Vec<String>,
    fixed: bool,
    /// Where the published list of servers is read from.
    discovery: String,
}

pub struct Directory {
    http: Http,
    mirrors: Mutex<Mirrors>,
    tags: Mutex<Option<TagsCache>>,
    tags_ttl: Duration,
    cache_file: Option<PathBuf>,
}

impl Default for Directory {
    fn default() -> Self {
        Self::new()
    }
}

impl Directory {
    /// A directory that finds its servers from the published list on first use.
    pub fn new() -> Self {
        Self::build(
            HttpConfig::default(),
            Mirrors {
                bases: Vec::new(),
                fixed: false,
                discovery: DISCOVERY_URL.to_owned(),
            },
        )
    }

    /// A directory that talks only to the given servers, as base addresses with no trailing slash
    /// (`https://de1.api.radio-browser.info`), trying them in order.
    pub fn with_mirrors(bases: Vec<String>) -> Self {
        Self::build(
            HttpConfig::default(),
            Mirrors {
                bases,
                fixed: true,
                discovery: DISCOVERY_URL.to_owned(),
            },
        )
    }

    pub fn with_config(mut self, config: HttpConfig) -> Self {
        self.http = Http::new(config, CancelHandle::new());
        self
    }

    /// Keeps the tag list in this file between runs, so a restart within the day does not refetch.
    pub fn with_cache_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.cache_file = Some(path.into());
        self
    }

    /// How long [`tags`](Self::tags) are kept before they are fetched again. A day by default.
    pub fn with_tags_ttl(mut self, ttl: Duration) -> Self {
        self.tags_ttl = ttl;
        self
    }

    fn build(config: HttpConfig, mirrors: Mirrors) -> Self {
        Self {
            http: Http::new(config, CancelHandle::new()),
            mirrors: Mutex::new(mirrors),
            tags: Mutex::new(None),
            tags_ttl: DAY,
            cache_file: None,
        }
    }

    /// The most-used tags, by station count, for browsing beyond the fixed genres.
    pub fn tags(&self) -> Result<Vec<Tag>> {
        let mut cached = self
            .tags
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cached.is_none() {
            *cached = self.read_cache_file();
        }
        let now = unix_now();
        if let Some(cache) = cached
            .as_ref()
            .filter(|cache| now.saturating_sub(cache.fetched_at) < self.tags_ttl.as_secs())
        {
            return Ok(cache.tags.clone());
        }
        let query =
            format!("/json/tags?order=stationcount&reverse=true&hidebroken=true&limit={TAG_LIMIT}");
        let raw: Vec<RawTag> = self.get_json(&query)?;
        let tags: Vec<Tag> = raw
            .into_iter()
            .filter(|tag| !tag.name.trim().is_empty())
            .map(|tag| Tag {
                name: tag.name,
                station_count: tag.stationcount,
            })
            .collect();
        let cache = TagsCache {
            fetched_at: now,
            tags: tags.clone(),
        };
        self.write_cache_file(&cache);
        *cached = Some(cache);
        Ok(tags)
    }

    /// A page of the genre's stations, most voted first, without those the directory has seen
    /// fail. A genre with several tags merges their lists.
    pub fn stations_by_genre(
        &self,
        genre: &Genre,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Station>> {
        let limit = limit.min(MAX_PAGE);
        // Each tag's list is sorted, so the top `offset + limit` of each contain the merged page.
        let depth = offset.saturating_add(limit).min(MAX_PAGE * 4);
        let lists = thread::scope(|scope| {
            let handles: Vec<_> = genre
                .tags
                .iter()
                .map(|tag| scope.spawn(move || self.stations_by_tag(tag, depth)))
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err(Error::NoMirror("a request panicked".to_owned())))
                })
                .collect::<Vec<_>>()
        });

        // One tag failing is fine as long as another answered.
        let mut merged: Vec<Station> = Vec::new();
        let mut first_error = None;
        let mut answered = false;
        for list in lists {
            match list {
                Ok(stations) => {
                    answered = true;
                    for station in stations {
                        if !merged.iter().any(|seen| seen.uuid == station.uuid) {
                            merged.push(station);
                        }
                    }
                }
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
        if !answered && let Some(error) = first_error {
            return Err(error);
        }
        merged.sort_by(|a, b| b.votes.cmp(&a.votes).then_with(|| a.name.cmp(&b.name)));
        Ok(merged
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    fn stations_by_tag(&self, tag: &str, limit: u32) -> Result<Vec<Station>> {
        let query = Self::query(
            "/json/stations/search",
            &[("tag", tag), ("tagExact", "true")],
            limit,
            0,
        );
        self.stations(&query)
    }

    /// Stations whose name contains `name`, most voted first.
    pub fn search(&self, name: &str, limit: u32) -> Result<Vec<Station>> {
        let query = Self::query(
            "/json/stations/search",
            &[("name", name.trim())],
            limit.min(MAX_PAGE),
            0,
        );
        self.stations(&query)
    }

    /// Tells the directory a station was played, which is how its popularity is counted. Call it
    /// when the user starts a station from the directory, not on reconnects or for their own URLs.
    pub fn count_click(&self, uuid: &str) -> Result<()> {
        // The identifier goes into the path, so it must be what it claims to be.
        if uuid.is_empty() || !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Err(Error::Response {
                what: "station id",
                detail: format!("{uuid:?} is not a station id"),
            });
        }
        let click: RawClick = self.get_json(&format!("/json/url/{uuid}"))?;
        if click.ok {
            Ok(())
        } else {
            Err(Error::Response {
                what: "click count",
                detail: click.message,
            })
        }
    }

    fn query(path: &str, filters: &[(&str, &str)], limit: u32, offset: u32) -> String {
        let mut serializer = form_urlencoded::Serializer::new(String::new());
        serializer.extend_pairs(filters);
        serializer.extend_pairs([
            ("order", "votes"),
            ("reverse", "true"),
            ("hidebroken", "true"),
        ]);
        serializer.append_pair("limit", &limit.to_string());
        serializer.append_pair("offset", &offset.to_string());
        format!("{path}?{}", serializer.finish())
    }

    fn stations(&self, query: &str) -> Result<Vec<Station>> {
        let raw: Vec<RawStation> = self.get_json(query)?;
        Ok(raw
            .into_iter()
            .filter_map(RawStation::into_station)
            .collect())
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path_and_query: &str) -> Result<T> {
        let (mirror, body) = self.get(path_and_query)?;
        serde_json::from_slice(&body).map_err(|error| Error::Response {
            what: "directory answer",
            detail: format!("{mirror}: {error}"),
        })
    }

    /// GETs from the first mirror that answers, which then becomes the first to try next time.
    fn get(&self, path_and_query: &str) -> Result<(String, Vec<u8>)> {
        let bases = self.bases();
        let mut last = None;
        for base in &bases {
            let url = Url::parse(&format!("{base}{path_and_query}"))
                .map_err(|_| Error::InvalidUrl(format!("{base}{path_and_query}")))?;
            match self.http.get_document(&url, MAX_RESPONSE_BYTES) {
                Ok((_, body)) => {
                    self.prefer(base);
                    return Ok((base.clone(), body));
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) => last = Some(error),
            }
        }
        Err(Error::NoMirror(last.map_or_else(
            || "no servers known".to_owned(),
            |error| error.to_string(),
        )))
    }

    /// The mirrors to try, discovering them first if that has not happened yet.
    fn bases(&self) -> Vec<String> {
        {
            let mirrors = self
                .mirrors
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if mirrors.fixed || !mirrors.bases.is_empty() {
                return mirrors.bases.clone();
            }
        }
        let mut discovered = self.discover();
        shuffle(&mut discovered);
        // The round-robin address answers too, and is the last resort.
        discovered.push(self.discovery_url());
        let mut mirrors = self
            .mirrors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if mirrors.bases.is_empty() {
            mirrors.bases = discovered;
        }
        mirrors.bases.clone()
    }

    fn discovery_url(&self) -> String {
        self.mirrors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .discovery
            .clone()
    }

    fn discover(&self) -> Vec<String> {
        let listed = Url::parse(&format!("{}/json/servers", self.discovery_url()))
            .ok()
            .and_then(|url| self.http.get_document(&url, MAX_RESPONSE_BYTES).ok())
            .and_then(|(_, body)| serde_json::from_slice::<Vec<RawServer>>(&body).ok())
            .unwrap_or_default();
        let mut bases: Vec<String> = Vec::new();
        for server in listed {
            let base = format!("https://{}", server.name.trim());
            if !server.name.trim().is_empty() && !bases.contains(&base) {
                bases.push(base);
            }
        }
        if bases.is_empty() {
            bases = FALLBACK_MIRRORS.map(str::to_owned).to_vec();
        }
        bases
    }

    fn prefer(&self, base: &str) {
        let mut mirrors = self
            .mirrors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(position) = mirrors
            .bases
            .iter()
            .position(|known| known == base)
            .filter(|&position| position > 0)
        {
            let preferred = mirrors.bases.remove(position);
            mirrors.bases.insert(0, preferred);
        }
    }

    fn read_cache_file(&self) -> Option<TagsCache> {
        let bytes = fs::read(self.cache_file.as_ref()?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// A cache that cannot be written only costs a refetch next run.
    fn write_cache_file(&self, cache: &TagsCache) {
        if let Some(path) = &self.cache_file
            && let Ok(bytes) = serde_json::to_vec(cache)
        {
            let _ = fs::write(path, bytes);
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// A shuffle seeded from the clock: it only has to spread load across mirrors, not be unpredictable.
fn shuffle(items: &mut [String]) {
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |elapsed| elapsed.as_nanos() as u64)
        | 1;
    for index in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        items.swap(index, (state % (index as u64 + 1)) as usize);
    }
}

#[cfg(test)]
mod tests;
