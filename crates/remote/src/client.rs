use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use ureq::Body;
use ureq::http::Response;
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::{Connector, DefaultConnector};

use crate::error::{Error, Result};
use crate::model::{
    Album, AlbumDetail, AlbumListKind, Artist, ArtistDetail, Playlist, PlaylistDetail,
    SearchResults, Song, SongListing,
};
use crate::stall::StallConnector;

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "AntiStreaming";
/// A JSON listing of a very large library is a few megabytes.
const DOCUMENT_LIMIT: u64 = 64 * 1024 * 1024;
const COVER_ART_LIMIT: u64 = 32 * 1024 * 1024;
const API_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const DEFAULT_CACHE_LIMIT: u64 = 1024 * 1024 * 1024;
const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(20);
/// Subsonic's own maximum for a page of `getAlbumList2`.
const ALBUMS_PER_PAGE: u32 = 500;
/// What clients send to `search3` to mean "everything": an empty phrase in quotes.
const MATCH_ALL: &str = "\"\"";

// What `Client::songs` has learned about the server.
const LISTING_UNKNOWN: u8 = 0;
const LISTING_SEARCH: u8 = 1;
const LISTING_ALBUM_WALK: u8 = 2;

/// A connection to one Subsonic-compatible server: another AntiStreaming, Navidrome, Airsonic.
/// Calls block; the app runs them off its main thread.
pub struct Client {
    pub(crate) base_url: String,
    pub(crate) user: String,
    password: String,
    /// For documents and images, which servers may take a while to produce.
    pub(crate) agent: ureq::Agent,
    /// For songs, which can take hours so long as bytes keep arriving.
    pub(crate) download_agent: ureq::Agent,
    stall_timeout: Duration,
    pub(crate) cache_limit: u64,
    listing: AtomicU8,
}

impl Client {
    /// `base_url` is the server's root, such as `http://192.168.1.20:4533`; the `/rest` part is
    /// added here. Nothing is sent until the first call.
    pub fn new(base_url: &str, user: &str, password: &str) -> Self {
        let config = base_config()
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .build();
        Self {
            base_url: base_url.trim().trim_end_matches('/').to_owned(),
            user: user.to_owned(),
            password: password.to_owned(),
            agent: ureq::Agent::new_with_config(config),
            download_agent: download_agent(DEFAULT_STALL_TIMEOUT),
            stall_timeout: DEFAULT_STALL_TIMEOUT,
            cache_limit: DEFAULT_CACHE_LIMIT,
            listing: AtomicU8::new(LISTING_UNKNOWN),
        }
    }

    /// How long a song download may go without receiving a byte before it fails with
    /// [`Error::Stalled`]. It bounds each wait, never the whole transfer. The default is 20
    /// seconds.
    pub fn with_stall_timeout(mut self, timeout: Duration) -> Self {
        self.stall_timeout = timeout;
        self.download_agent = download_agent(timeout);
        self
    }

    /// Caps the total size of the files `download_to_cache` keeps. The default is 1 GiB.
    pub fn with_cache_limit(mut self, bytes: u64) -> Self {
        self.cache_limit = bytes;
        self
    }

    /// Checks that the server is there and accepts these credentials.
    pub fn ping(&self) -> Result<()> {
        self.call("ping", &[]).map(drop)
    }

    /// What the server calls itself, ready for a sidebar: OpenSubsonic servers report their
    /// product and version ("navidrome 0.52.5", "AntiStreaming 0.1.0"); a classic Subsonic one
    /// reports neither and is just "Subsonic".
    pub fn server_name(&self) -> Result<String> {
        let reply = self.call("ping", &[])?;
        let text = |key: &str| {
            reply
                .get(key)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
        };
        Ok(match (text("type"), text("serverVersion")) {
            (Some(product), Some(version)) => format!("{product} {version}"),
            (Some(product), None) => product.to_owned(),
            _ => "Subsonic".to_owned(),
        })
    }

    /// Every artist, in the server's alphabetical order.
    pub fn artists(&self) -> Result<Vec<Artist>> {
        #[derive(Deserialize, Default)]
        struct Indexes {
            #[serde(default)]
            index: Vec<Group>,
        }
        #[derive(Deserialize)]
        struct Group {
            #[serde(default)]
            artist: Vec<Artist>,
        }
        let indexes: Indexes = self.fetch_or_empty("getArtists", &[], "artists")?;
        Ok(indexes
            .index
            .into_iter()
            .flat_map(|group| group.artist)
            .collect())
    }

    pub fn artist(&self, id: &str) -> Result<ArtistDetail> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(flatten)]
            artist: Artist,
            #[serde(default)]
            album: Vec<Album>,
        }
        let wire: Wire = self.fetch("getArtist", &[("id", id)], "artist")?;
        Ok(ArtistDetail {
            artist: wire.artist,
            albums: wire.album,
        })
    }

    pub fn album(&self, id: &str) -> Result<AlbumDetail> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(flatten)]
            album: Album,
            #[serde(default)]
            song: Vec<Song>,
        }
        let wire: Wire = self.fetch("getAlbum", &[("id", id)], "album")?;
        Ok(AlbumDetail {
            album: wire.album,
            songs: wire.song,
        })
    }

    pub fn album_list(&self, kind: AlbumListKind, size: u32, offset: u32) -> Result<Vec<Album>> {
        #[derive(Deserialize, Default)]
        struct Wire {
            #[serde(default)]
            album: Vec<Album>,
        }
        let (size, offset) = (size.to_string(), offset.to_string());
        let params = [
            ("type", kind.as_param()),
            ("size", size.as_str()),
            ("offset", offset.as_str()),
        ];
        let wire: Wire = self.fetch_or_empty("getAlbumList2", &params, "albumList2")?;
        Ok(wire.album)
    }

    pub fn search(&self, query: &str) -> Result<SearchResults> {
        let wire = self.search3(&[("query", query)])?;
        Ok(SearchResults {
            artists: wire.artist,
            albums: wire.album,
            songs: wire.song,
        })
    }

    /// Up to `count` songs of the whole library, starting `offset` songs in, for a full sync.
    /// Page through it by adding what came back to `offset` until nothing does; a server may
    /// return fewer than `count`.
    ///
    /// It asks `search3` for everything, with an empty query, which is how Symfonium syncs and
    /// what Navidrome supports. A server that refuses that (it answers with a Subsonic error, or
    /// with nothing at all to a library that has albums) is handled by listing albums
    /// alphabetically and opening them one at a time instead, which takes a request per album
    /// rather than per page, and which pages by the albums' own song counts. The client
    /// remembers the outcome, so only the first call pays for finding out; [`Client::song_listing`]
    /// reports which way it went. Real failures, such as a wrong password or an unreachable
    /// server, are errors, never a reason to switch.
    pub fn songs(&self, offset: u32, count: u32) -> Result<Vec<Song>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let known = self.listing.load(Ordering::Relaxed);
        if known != LISTING_ALBUM_WALK {
            match self.search_songs(offset, count) {
                Ok(songs) if !songs.is_empty() => {
                    self.listing.store(LISTING_SEARCH, Ordering::Relaxed);
                    return Ok(songs);
                }
                // Past the end, or a library that has since emptied.
                Ok(songs) if offset > 0 || known == LISTING_SEARCH => return Ok(songs),
                // Nothing at the very start: no songs, or a server that does not do "everything".
                Ok(_) => {}
                Err(Error::Server { .. }) if known == LISTING_UNKNOWN => {}
                Err(error) => return Err(error),
            }
        }
        let songs = self.walk_albums(offset, count)?;
        if !songs.is_empty() {
            self.listing.store(LISTING_ALBUM_WALK, Ordering::Relaxed);
        }
        Ok(songs)
    }

    /// How [`Client::songs`] has been getting its songs from this server, once it has found
    /// songs; `None` before that.
    pub fn song_listing(&self) -> Option<SongListing> {
        match self.listing.load(Ordering::Relaxed) {
            LISTING_SEARCH => Some(SongListing::Search),
            LISTING_ALBUM_WALK => Some(SongListing::AlbumWalk),
            _ => None,
        }
    }

    fn search_songs(&self, offset: u32, count: u32) -> Result<Vec<Song>> {
        let (count, offset) = (count.to_string(), offset.to_string());
        let params = [
            ("query", MATCH_ALL),
            ("artistCount", "0"),
            ("albumCount", "0"),
            ("songCount", count.as_str()),
            ("songOffset", offset.as_str()),
        ];
        Ok(self.search3(&params)?.song)
    }

    /// Lists albums in order and opens only those that overlap the wanted window; albums wholly
    /// before it are skipped using the song count the listing gives, so they cost no request.
    fn walk_albums(&self, offset: u32, count: u32) -> Result<Vec<Song>> {
        let wanted = u64::from(offset)..u64::from(offset) + u64::from(count);
        let mut songs = Vec::new();
        let mut position = 0u64;
        let mut listed = 0;
        loop {
            let page =
                self.album_list(AlbumListKind::AlphabeticalByName, ALBUMS_PER_PAGE, listed)?;
            for album in &page {
                if position + u64::from(album.song_count) <= wanted.start {
                    position += u64::from(album.song_count);
                    continue;
                }
                for song in self.album(&album.id)?.songs {
                    if wanted.contains(&position) {
                        songs.push(song);
                    }
                    position += 1;
                }
                if position >= wanted.end {
                    return Ok(songs);
                }
            }
            if page.len() < ALBUMS_PER_PAGE as usize {
                return Ok(songs);
            }
            listed += ALBUMS_PER_PAGE;
        }
    }

    fn search3(&self, params: &[(&str, &str)]) -> Result<SearchWire> {
        self.fetch_or_empty("search3", params, "searchResult3")
    }

    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        #[derive(Deserialize, Default)]
        struct Wire {
            #[serde(default)]
            playlist: Vec<Playlist>,
        }
        let wire: Wire = self.fetch_or_empty("getPlaylists", &[], "playlists")?;
        Ok(wire.playlist)
    }

    pub fn playlist(&self, id: &str) -> Result<PlaylistDetail> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(flatten)]
            playlist: Playlist,
            #[serde(default)]
            entry: Vec<Song>,
        }
        let wire: Wire = self.fetch("getPlaylist", &[("id", id)], "playlist")?;
        Ok(PlaylistDetail {
            playlist: wire.playlist,
            songs: wire.entry,
        })
    }

    /// The image bytes, in whatever format the server holds; `size` asks for a square of about
    /// that many pixels.
    pub fn cover_art(&self, id: &str, size: Option<u32>) -> Result<Vec<u8>> {
        let size = size.map(|pixels| pixels.to_string());
        let mut params = vec![("id", id)];
        if let Some(size) = &size {
            params.push(("size", size));
        }
        let mut response = self.agent.get(self.url("getCoverArt", &params)).call()?;
        check_media(&mut response)?;
        Ok(response
            .body_mut()
            .with_config()
            .limit(COVER_ART_LIMIT)
            .read_to_vec()?)
    }

    /// A URL the player can stream the song from. It carries a one-off login token, so anyone who
    /// sees the URL can replay it, but it does not reveal the password. `max_bitrate` is in
    /// kilobits per second; `None` or 0 means the file as stored.
    pub fn stream_url(&self, song_id: &str, max_bitrate: Option<u32>) -> String {
        let max_bitrate = max_bitrate.unwrap_or(0).to_string();
        self.url("stream", &[("id", song_id), ("maxBitRate", &max_bitrate)])
    }

    /// Tells the server a song is playing (`submission` false) or has been played (true), which
    /// is what feeds its play counts.
    pub fn scrobble(&self, id: &str, submission: bool) -> Result<()> {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis())
            .to_string();
        let submission = submission.to_string();
        self.call(
            "scrobble",
            &[("id", id), ("time", &time), ("submission", &submission)],
        )
        .map(drop)
    }

    /// The full request URL, authenticated with a fresh token. Each call gets its own salt, as
    /// the protocol requires.
    pub(crate) fn url(&self, endpoint: &str, params: &[(&str, &str)]) -> String {
        let salt = fresh_salt();
        let token = hex::encode(Md5::digest(format!("{}{salt}", self.password)));
        let mut query = form_urlencoded::Serializer::new(String::new());
        query
            .append_pair("u", &self.user)
            .append_pair("t", &token)
            .append_pair("s", &salt)
            .append_pair("v", API_VERSION)
            .append_pair("c", CLIENT_NAME)
            .append_pair("f", "json");
        for (name, value) in params {
            query.append_pair(name, value);
        }
        format!("{}/rest/{endpoint}?{}", self.base_url, query.finish())
    }

    /// Makes a request and returns the members of the `subsonic-response` other than its status.
    fn call(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<Map<String, Value>> {
        let mut response = self
            .agent
            .get(self.url(endpoint, params))
            .config()
            .timeout_global(Some(API_TIMEOUT))
            .build()
            .call()?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(DOCUMENT_LIMIT)
            .read_to_string()?;
        match parse_envelope(&body) {
            // A wrong URL tends to answer with a web page or a bare HTTP error.
            Err(Error::Unexpected(_)) if !(200..300).contains(&status) => {
                Err(Error::Status(status))
            }
            parsed => parsed,
        }
    }

    fn fetch<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
        key: &str,
    ) -> Result<T> {
        let mut payload = self.call(endpoint, params)?;
        let value = payload
            .remove(key)
            .ok_or_else(|| Error::Unexpected(format!("the response has no {key}")))?;
        Ok(serde_json::from_value(value)?)
    }

    /// For listings, which servers may leave out entirely when there is nothing to list.
    fn fetch_or_empty<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
        key: &str,
    ) -> Result<T> {
        let mut payload = self.call(endpoint, params)?;
        let value = payload
            .remove(key)
            .unwrap_or_else(|| Value::Object(Map::new()));
        Ok(serde_json::from_value(value)?)
    }
}

#[derive(Deserialize, Default)]
struct SearchWire {
    #[serde(default)]
    artist: Vec<Artist>,
    #[serde(default)]
    album: Vec<Album>,
    #[serde(default)]
    song: Vec<Song>,
}

fn base_config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    ureq::Agent::config_builder()
        .user_agent(concat!("AntiStreaming/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(10)))
        // Media endpoints answer with plain HTTP statuses that callers branch on themselves.
        .http_status_as_error(false)
}

/// `stall_timeout` bounds each wait for data, not the whole transfer, so no global or body
/// timeout is configured.
fn download_agent(stall_timeout: Duration) -> ureq::Agent {
    let config = base_config()
        .timeout_recv_response(Some(stall_timeout))
        .build();
    ureq::Agent::with_parts(
        config,
        DefaultConnector::new().chain(StallConnector::new(stall_timeout)),
        DefaultResolver::default(),
    )
}

/// Turns the body of a Subsonic JSON reply into its payload, or the error it carries.
fn parse_envelope(body: &str) -> Result<Map<String, Value>> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "subsonic-response")]
        response: Response,
    }
    #[derive(Deserialize)]
    struct Response {
        status: String,
        error: Option<ServerError>,
        #[serde(flatten)]
        payload: Map<String, Value>,
    }
    #[derive(Deserialize)]
    struct ServerError {
        code: u32,
        #[serde(default)]
        message: String,
    }

    let envelope: Envelope = serde_json::from_str(body)
        .map_err(|_| Error::Unexpected("the server did not answer in Subsonic JSON".to_owned()))?;
    let response = envelope.response;
    match response.error {
        Some(error) => Err(Error::from_subsonic(error.code, error.message)),
        None if response.status == "ok" => Ok(response.payload),
        None => Err(Error::Server {
            code: 0,
            message: format!("status {}", response.status),
        }),
    }
}

/// Media endpoints answer with the file itself, or with a Subsonic error document, or with a bare
/// HTTP error. This turns the last two into errors and lets a file through.
pub(crate) fn check_media(response: &mut Response<Body>) -> Result<()> {
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.contains("json") {
        let body = response
            .body_mut()
            .with_config()
            .limit(DOCUMENT_LIMIT)
            .read_to_string()?;
        return Err(match parse_envelope(&body) {
            Err(error) => error,
            Ok(_) => {
                Error::Unexpected("the server answered a media request with a document".to_owned())
            }
        });
    }
    if content_type.contains("xml") {
        return Err(Error::Unexpected(
            "the server answered in XML although JSON was asked for".to_owned(),
        ));
    }
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(Error::NotAuthorized),
        404 => Err(Error::NotFound),
        other => Err(Error::Status(other)),
    }
}

/// Unique per request, which is all a salt has to be; it protects nothing by being secret.
fn fresh_salt() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let state = RandomState::new();
    format!(
        "{:016x}{:016x}",
        state.hash_one(nanos),
        state.hash_one(nanos + 1)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_give_up_after_twenty_quiet_seconds_unless_told_otherwise() {
        let client = Client::new("http://example.test", "u", "p");
        assert_eq!(client.stall_timeout, Duration::from_secs(20));

        let client = client.with_stall_timeout(Duration::from_secs(5));
        assert_eq!(client.stall_timeout, Duration::from_secs(5));
    }
}
