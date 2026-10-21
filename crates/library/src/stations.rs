use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{MediaKind, RADIO, Track};
use crate::scan::now_unix;
use crate::store::{WriteMode, write_tracks};

const MAX_URL_BYTES: usize = 2048;
const MAX_TEXT_CHARS: usize = 200;
const MAX_CODEC_CHARS: usize = 20;
const MAX_UUID_CHARS: usize = 64;
/// A station list has a few dozen entries; a file with thousands is not one.
const MAX_IMPORT_ENTRIES: usize = 1_000;
const MAX_PLAYLIST_BYTES: u64 = 1024 * 1024;

/// A station as the user or the directory describes it. The name, address and
/// the rest are cleaned on the way in: addresses must be http or https, text is
/// cut to a sensible length, and details that are not usable are dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewStation {
    /// Shown as the title; the address stands in when it is empty.
    pub name: String,
    /// The stream's address, kept as the track's `path`.
    pub url: String,
    pub genre: String,
    pub bitrate_kbps: Option<u32>,
    /// What the stream is encoded as, e.g. "MP3", "AAC".
    pub codec: String,
    pub homepage: Option<String>,
    pub favicon_url: Option<String>,
    /// The station's id in the Radio Browser directory, when it came from there.
    pub radio_browser_uuid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    /// The track id, which playlists and play counts use.
    pub id: i64,
    pub name: String,
    pub url: String,
    pub genre: String,
    pub bitrate_kbps: Option<u32>,
    pub codec: String,
    pub homepage: Option<String>,
    pub favicon_url: Option<String>,
    pub radio_browser_uuid: Option<String>,
    pub play_count: u32,
    pub last_played: Option<i64>,
}

/// The id of a station that is not in the library: no track has it.
const UNSAVED_ID: i64 = -1;

impl Station {
    /// `station` as it would be kept, for playing it without keeping it.
    pub fn unsaved(station: &NewStation) -> Result<Self> {
        let station = Clean::new(station)?;
        Ok(Self {
            id: UNSAVED_ID,
            name: station.name,
            url: station.url,
            genre: station.genre,
            bitrate_kbps: station.bitrate_kbps,
            codec: station.codec,
            homepage: station.homepage,
            favicon_url: station.favicon_url,
            radio_browser_uuid: station.uuid,
            play_count: 0,
            last_played: None,
        })
    }

    pub fn is_unsaved(&self) -> bool {
        self.id == UNSAVED_ID
    }
}

impl From<&Station> for NewStation {
    fn from(station: &Station) -> Self {
        Self {
            name: station.name.clone(),
            url: station.url.clone(),
            genre: station.genre.clone(),
            bitrate_kbps: station.bitrate_kbps,
            codec: station.codec.clone(),
            homepage: station.homepage.clone(),
            favicon_url: station.favicon_url.clone(),
            radio_browser_uuid: station.radio_browser_uuid.clone(),
        }
    }
}

/// A `NewStation` that has been checked and cleaned.
struct Clean {
    name: String,
    url: String,
    genre: String,
    bitrate_kbps: Option<u32>,
    codec: String,
    homepage: Option<String>,
    favicon_url: Option<String>,
    uuid: Option<String>,
}

impl Clean {
    fn new(station: &NewStation) -> Result<Self> {
        let url = web_url(&station.url).ok_or_else(|| {
            Error::InvalidStation("the address must be an http or https URL".to_owned())
        })?;
        let name = text(&station.name, MAX_TEXT_CHARS);
        Ok(Self {
            name: if name.is_empty() {
                text(&url, MAX_TEXT_CHARS)
            } else {
                name
            },
            url,
            genre: text(&station.genre, MAX_TEXT_CHARS),
            bitrate_kbps: station.bitrate_kbps.filter(|kbps| *kbps > 0),
            codec: text(&station.codec, MAX_CODEC_CHARS),
            homepage: station.homepage.as_deref().and_then(web_url),
            favicon_url: station.favicon_url.as_deref().and_then(web_url),
            uuid: station
                .radio_browser_uuid
                .as_deref()
                .map(str::trim)
                .filter(|uuid| {
                    !uuid.is_empty()
                        && uuid.len() <= MAX_UUID_CHARS
                        && uuid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                })
                .map(str::to_owned),
        })
    }
}

/// The address with its scheme in lower case, if it is an http or https one
/// with a host and nothing a stray control character or space could hide in.
fn web_url(text: &str) -> Option<String> {
    let text = text.trim();
    let (scheme, rest) = text.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https")
        || text.len() > MAX_URL_BYTES
        || text.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    (!host.is_empty() && !host.starts_with(':')).then(|| format!("{scheme}://{rest}"))
}

/// One line of text: whitespace collapsed, control characters gone, cut short.
fn text(text: &str, max_chars: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_owned()
}

const STATIONS: &str = "SELECT t.id, t.title, t.path, t.genre, t.bitrate_kbps, t.codec,
           s.homepage, s.favicon_url, s.radio_browser_uuid, t.play_count, t.last_played
    FROM tracks t LEFT JOIN stations s ON s.track_id = t.id";

fn station_from_row(row: &Row<'_>) -> rusqlite::Result<Station> {
    Ok(Station {
        id: row.get(0)?,
        name: row.get(1)?,
        url: row.get(2)?,
        genre: row.get(3)?,
        bitrate_kbps: row.get(4)?,
        codec: row.get(5)?,
        homepage: row.get(6)?,
        favicon_url: row.get(7)?,
        radio_browser_uuid: row.get(8)?,
        play_count: row.get(9)?,
        last_played: row.get(10)?,
    })
}

impl Library {
    /// Adds a station and returns its id, or the id of the one already here:
    /// a station with the same directory id, else the same address, is the
    /// same station and is left as it is. Stations are tracks of kind radio,
    /// so playlists, play counts and the rest work as for songs.
    pub fn add_station(&mut self, station: &NewStation) -> Result<i64> {
        let station = Clean::new(station)?;
        let tx = self.conn.transaction()?;
        let id = insert_station(&tx, &station)?;
        tx.commit()?;
        Ok(id)
    }

    /// The station already here that `add_station` would give `station`'s id to.
    pub fn kept_station(&self, station: &NewStation) -> Result<Option<Station>> {
        match kept_id(&self.conn, &Clean::new(station)?)? {
            Some(id) => self.station(id),
            None => Ok(None),
        }
    }

    /// Replaces a station's details, keeping its place in playlists and its
    /// play history. The address and directory id must not belong to another
    /// station.
    pub fn update_station(&mut self, id: i64, station: &NewStation) -> Result<()> {
        let station = Clean::new(station)?;
        let mut track = self
            .track(id)?
            .filter(|track| track.kind == MediaKind::Radio)
            .ok_or(Error::TrackNotFound(id))?;
        let tx = self.conn.transaction()?;
        let url_taken = tx
            .query_row(
                "SELECT 1 FROM tracks WHERE path = ?1 AND id <> ?2",
                params![station.url, id],
                |_| Ok(()),
            )
            .optional()?;
        if url_taken.is_some() {
            return Err(Error::PathInUse(station.url));
        }
        if let Some(uuid) = &station.uuid {
            let uuid_taken = tx
                .query_row(
                    "SELECT 1 FROM stations WHERE radio_browser_uuid = ?1 AND track_id <> ?2",
                    params![uuid, id],
                    |_| Ok(()),
                )
                .optional()?;
            if uuid_taken.is_some() {
                return Err(Error::InvalidStation(
                    "another station already has this directory id".to_owned(),
                ));
            }
        }
        // The writer upserts by path, so the row has to be at its new one first.
        tx.execute(
            "UPDATE tracks SET path = ?2 WHERE id = ?1",
            params![id, station.url],
        )?;
        track.path.clone_from(&station.url);
        track.title.clone_from(&station.name);
        track.genre.clone_from(&station.genre);
        track.bitrate_kbps = station.bitrate_kbps;
        track.codec.clone_from(&station.codec);
        write_tracks(&tx, WriteMode::RefreshTags, [&track])?;
        tx.execute(
            "INSERT INTO stations (track_id, homepage, favicon_url, radio_browser_uuid)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(track_id) DO UPDATE SET homepage = excluded.homepage,
                 favicon_url = excluded.favicon_url,
                 radio_browser_uuid = excluded.radio_browser_uuid",
            params![id, station.homepage, station.favicon_url, station.uuid],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Removes a station, and with it its place in every playlist. An id that
    /// is not a station's is not found, so a song cannot be removed by it.
    pub fn remove_station(&mut self, id: i64) -> Result<()> {
        let removed = self.conn.execute(
            &format!("DELETE FROM tracks WHERE id = ?1 AND kind = {RADIO}"),
            [id],
        )?;
        if removed == 0 {
            return Err(Error::TrackNotFound(id));
        }
        Ok(())
    }

    pub fn station(&self, id: i64) -> Result<Option<Station>> {
        Ok(self
            .conn
            .query_row(
                &format!("{STATIONS} WHERE t.kind = {RADIO} AND t.id = ?1"),
                [id],
                station_from_row,
            )
            .optional()?)
    }

    /// Every station, by name.
    pub fn stations(&self) -> Result<Vec<Station>> {
        let mut stmt = self.conn.prepare(&format!(
            "{STATIONS} WHERE t.kind = {RADIO} ORDER BY t.key_title NULLS LAST, t.id"
        ))?;
        let rows = stmt.query_map([], station_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The last stations played, most recent first; one that was never played
    /// is not listed.
    pub fn recently_played_stations(&self, limit: usize) -> Result<Vec<Station>> {
        let mut stmt = self.conn.prepare(&format!(
            "{STATIONS} WHERE t.kind = {RADIO} AND t.last_played IS NOT NULL
             ORDER BY t.last_played DESC, t.id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], station_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Adds the stations a `.pls` or `.m3u` file lists and returns their ids in
    /// the file's order, each once, including stations that were already here.
    /// A file is untrusted: only the first megabyte is read, only http and
    /// https addresses become stations (file paths and other schemes are
    /// skipped), and no more than a thousand are taken. A file that lists
    /// none, such as a playlist of songs or an HLS manifest, adds none.
    pub fn import_playlist_file(&mut self, path: &Path) -> Result<Vec<i64>> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_PLAYLIST_BYTES)
            .read_to_end(&mut bytes)?;
        let entries: Vec<Clean> = station_entries(&String::from_utf8_lossy(&bytes))
            .iter()
            .filter_map(|entry| Clean::new(entry).ok())
            .take(MAX_IMPORT_ENTRIES)
            .collect();

        let tx = self.conn.transaction()?;
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for entry in &entries {
            let id = insert_station(&tx, entry)?;
            if seen.insert(id) {
                ids.push(id);
            }
        }
        tx.commit()?;
        Ok(ids)
    }
}

/// The id of the station that `station` is: the one with its directory id, else the one at its
/// address.
fn kept_id(conn: &Connection, station: &Clean) -> Result<Option<i64>> {
    if let Some(uuid) = &station.uuid {
        let known: Option<i64> = conn
            .query_row(
                "SELECT track_id FROM stations WHERE radio_browser_uuid = ?1",
                [uuid],
                |row| row.get(0),
            )
            .optional()?;
        if known.is_some() {
            return Ok(known);
        }
    }
    Ok(conn
        .query_row(
            &format!("SELECT id FROM tracks WHERE path = ?1 AND kind = {RADIO}"),
            [&station.url],
            |row| row.get(0),
        )
        .optional()?)
}

fn insert_station(tx: &Transaction<'_>, station: &Clean) -> Result<i64> {
    if let Some(id) = kept_id(tx, station)? {
        return Ok(id);
    }
    let taken: Option<i64> = tx
        .query_row(
            "SELECT id FROM tracks WHERE path = ?1",
            [&station.url],
            |row| row.get(0),
        )
        .optional()?;
    // Never overwrite a song that was imported with a web address as its path.
    if taken.is_some() {
        return Err(Error::PathInUse(station.url.clone()));
    }

    write_tracks(tx, WriteMode::RefreshTags, [&station_row(station)])?;
    let id: i64 = tx.query_row(
        "SELECT id FROM tracks WHERE path = ?1",
        [&station.url],
        |row| row.get(0),
    )?;
    tx.execute(
        "INSERT INTO stations (track_id, homepage, favicon_url, radio_browser_uuid)
         VALUES (?1, ?2, ?3, ?4)",
        params![id, station.homepage, station.favicon_url, station.uuid],
    )?;
    Ok(id)
}

fn station_row(station: &Clean) -> Track {
    let now = now_unix();
    Track {
        id: 0,
        path: station.url.clone(),
        title: station.name.clone(),
        artist: String::new(),
        album_artist: String::new(),
        album: String::new(),
        genre: station.genre.clone(),
        composer: String::new(),
        year: None,
        track_number: None,
        track_count: None,
        disc_number: None,
        disc_count: None,
        duration_ms: 0,
        bitrate_kbps: station.bitrate_kbps,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        codec: station.codec.clone(),
        file_size: 0,
        date_added: now,
        date_modified: now,
        play_count: 0,
        last_played: None,
        skip_count: 0,
        rating: 0,
        kind: MediaKind::Radio,
        compilation: false,
        enabled: true,
        in_cloud: false,
        missing: false,
        loudness: None,
        remember_position: false,
        has_lyrics: false,
        album_group: String::new(),
        sort_title: None,
        sort_artist: None,
        sort_album: None,
        sort_album_artist: None,
    }
}

/// The name and address of each entry in a `.pls` or `.m3u` text, whatever the
/// address is: the caller decides which are stations.
fn station_entries(text: &str) -> Vec<NewStation> {
    let text = text.trim_start_matches('\u{feff}');
    let first = text.lines().map(str::trim).find(|line| !line.is_empty());
    if first.is_some_and(|line| line.eq_ignore_ascii_case("[playlist]")) {
        pls_entries(text)
    } else {
        m3u_entries(text)
    }
}

/// `FileN=address` with the name in `TitleN=`. Entries go by their number, not
/// by where they sit in the file, and the declared count is not trusted.
fn pls_entries(text: &str) -> Vec<NewStation> {
    let mut entries: BTreeMap<u32, NewStation> = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if let Some(number) = key.strip_prefix("file").and_then(|n| n.parse().ok()) {
            entries.entry(number).or_default().url = value.trim().to_owned();
        } else if let Some(number) = key.strip_prefix("title").and_then(|n| n.parse().ok()) {
            entries.entry(number).or_default().name = value.trim().to_owned();
        }
    }
    entries.into_values().collect()
}

/// Each address line, named by the `#EXTINF:length,Name` line before it. HLS
/// manifests have the same shape but list pieces of one stream, not stations.
fn m3u_entries(text: &str) -> Vec<NewStation> {
    let mut entries = Vec::new();
    let mut name = String::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line.starts_with("#EXT-X-") {
            return Vec::new();
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            name = info
                .split_once(',')
                .map(|(_, name)| name.to_owned())
                .unwrap_or_default();
        } else if !line.starts_with('#') {
            entries.push(NewStation {
                name: std::mem::take(&mut name),
                url: line.to_owned(),
                ..NewStation::default()
            });
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_and_https_addresses_with_a_host_are_stations() {
        for (given, expected) in [
            (
                "http://ice.example.com:8000/live",
                "http://ice.example.com:8000/live",
            ),
            (
                "  HTTPS://Radio.Example/stream.mp3?x=1#y  ",
                "https://Radio.Example/stream.mp3?x=1#y",
            ),
            ("http://user:pw@host/a", "http://user:pw@host/a"),
            ("http://[::1]:8000/", "http://[::1]:8000/"),
            (
                "http://h\u{e9}.example/\u{e9}",
                "http://h\u{e9}.example/\u{e9}",
            ),
        ] {
            assert_eq!(web_url(given).as_deref(), Some(expected), "{given:?}");
        }
        for rejected in [
            "",
            "ice.example.com/live",
            "ftp://host/file",
            "file:///etc/passwd",
            "javascript://host/%0aalert(1)",
            "mms://host/stream",
            "rtsp://host/stream",
            "data:text/plain,hi",
            "http://",
            "https:///path",
            "http://:8000/",
            "http://user@/path",
            "http://host/with space",
            "http://host/tab\there",
            "http://host/nul\0",
            "http://host/new\nline",
            "//host/stream",
            "http:/host/stream",
        ] {
            assert_eq!(web_url(rejected), None, "{rejected:?}");
        }
        let long = format!("http://host/{}", "a".repeat(MAX_URL_BYTES));
        assert_eq!(web_url(&long), None);
    }

    #[test]
    fn text_is_one_clean_short_line() {
        assert_eq!(
            text("  Soma \t FM\r\n(Groove)\u{7}  ", 50),
            "Soma FM (Groove)"
        );
        assert_eq!(text("abcdef", 3), "abc");
        assert_eq!(text("\u{1b}[31m", 50), "[31m");
        assert_eq!(text(" \n ", 50), "");
    }

    #[test]
    fn details_that_are_not_usable_are_dropped_not_fatal() {
        let clean = Clean::new(&NewStation {
            name: String::new(),
            url: "http://ice.example/live".into(),
            genre: "x".repeat(1_000),
            bitrate_kbps: Some(0),
            codec: "AAC+ with a ridiculously long description".into(),
            homepage: Some("javascript:alert(1)".into()),
            favicon_url: Some("ftp://host/icon.png".into()),
            radio_browser_uuid: Some("not a uuid!".into()),
        })
        .unwrap();

        assert_eq!(clean.name, "http://ice.example/live");
        assert_eq!(clean.genre.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(clean.bitrate_kbps, None);
        assert_eq!(clean.codec.chars().count(), MAX_CODEC_CHARS);
        assert_eq!(
            (clean.homepage, clean.favicon_url, clean.uuid),
            (None, None, None)
        );
        assert!(Clean::new(&NewStation::default()).is_err());
    }

    #[test]
    fn pls_entries_go_by_number_and_survive_gaps_and_junk() {
        let entries = pls_entries(
            "[playlist]\nNumberOfEntries=99999\nFile2=http://b/\nTitle2=Second\n\
             garbage line\nfile1 = http://a/\nTITLE1= First \nTitle3=No address\n\
             FileX=http://never/\nFile4=\nVersion=2\n",
        );

        let summary: Vec<(&str, &str)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.url.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("First", "http://a/"),
                ("Second", "http://b/"),
                ("No address", ""),
                ("", "")
            ]
        );
    }

    #[test]
    fn m3u_entries_take_names_from_extinf_and_give_up_on_hls() {
        let entries = m3u_entries(
            "#EXTM3U\n#EXTINF:-1,Jazz FM, live\nhttp://a/\n\n#EXTINF:-1\nhttp://b/\n\
             #EXTINF:-1,Orphan name\n#COMMENT\nrelative/song.mp3\n",
        );
        let summary: Vec<(&str, &str)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.url.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("Jazz FM, live", "http://a/"),
                ("", "http://b/"),
                ("Orphan name", "relative/song.mp3")
            ]
        );

        assert!(m3u_entries("#EXTM3U\n#EXT-X-VERSION:3\nhttp://a/seg1.ts\n").is_empty());
    }

    #[test]
    fn the_format_is_told_by_content_not_extension() {
        assert_eq!(
            station_entries("\u{feff}\n[PlayList]\nFile1=http://a/\n").len(),
            1
        );
        assert_eq!(station_entries("http://a/\nhttp://b/\n").len(), 2);
        assert!(station_entries("").is_empty());
    }
}
