use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use percent_encoding::percent_decode_str;
use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::classify::{Evidence, classify};
use crate::error::Result;
use crate::item::{MediaType, MusicLibrary, MusicLibraryItem, MusicPlaylist, PlaylistKind};

/// The songs of `read_xml_library`, for callers that have no use for playlists.
pub fn read_xml(path: &Path) -> Result<Vec<MusicLibraryItem>> {
    Ok(read_xml_library(path)?.items)
}

/// Reads a library exported from Music (File > Library > Export Library...) or iTunes
/// ("iTunes Library.xml"). Songs and playlists without a persistent ID are skipped because
/// nothing stable identifies them, and so are playlist entries naming a song that was skipped
/// or is not in the export.
pub fn read_xml_library(path: &Path) -> Result<MusicLibrary> {
    let export: Export = plist::from_reader(BufReader::new(File::open(path)?))?;
    let mut items = Vec::with_capacity(export.tracks.len());
    // Playlists name songs by the export's own track ID, not by persistent ID.
    let mut persistent_ids = HashMap::with_capacity(export.tracks.len());
    for (track_id, track) in export.tracks {
        if let Some(item) = plist::from_value::<Track>(&track)?.into_item() {
            if let Ok(track_id) = track_id.parse::<u64>() {
                persistent_ids.insert(track_id, item.persistent_id.clone());
            }
            items.push(item);
        }
    }
    let playlists = export
        .playlists
        .into_iter()
        .filter_map(|playlist| playlist.into_playlist(&persistent_ids))
        .collect();
    Ok(MusicLibrary::new(items, playlists))
}

#[derive(Deserialize)]
struct Export {
    // A dictionary rather than a map type so tracks keep the order iTunes wrote them in.
    #[serde(rename = "Tracks", default)]
    tracks: plist::Dictionary,
    #[serde(rename = "Playlists", default)]
    playlists: Vec<Playlist>,
}

/// Only the keys we use; serde ignores everything else in the entry.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Track {
    #[serde(rename = "Persistent ID")]
    persistent_id: Option<String>,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Artist")]
    artist: String,
    #[serde(rename = "Album Artist")]
    album_artist: String,
    #[serde(rename = "Album")]
    album: String,
    #[serde(rename = "Genre")]
    genre: String,
    #[serde(rename = "Total Time")]
    total_time: Option<u64>,
    #[serde(rename = "Track Number")]
    track_number: Option<u32>,
    #[serde(rename = "Disc Number")]
    disc_number: Option<u32>,
    #[serde(rename = "Year")]
    year: Option<i32>,
    #[serde(rename = "Kind")]
    kind: String,
    #[serde(rename = "Location")]
    location: Option<String>,
    #[serde(rename = "Track Type")]
    track_type: Option<String>,
    #[serde(rename = "Play Count")]
    play_count: u32,
    #[serde(rename = "Skip Count")]
    skip_count: u32,
    #[serde(rename = "Rating")]
    rating: u8,
    #[serde(rename = "Date Added")]
    date_added: Option<plist::Date>,
    // The plain `Play Date` is local-time seconds on the Mac epoch; the UTC one is a real date.
    #[serde(rename = "Play Date UTC")]
    last_played: Option<plist::Date>,
    #[serde(rename = "Skip Date")]
    last_skipped: Option<plist::Date>,
    // Written only when the song is unchecked.
    #[serde(rename = "Disabled")]
    disabled: bool,
    #[serde(rename = "Apple Music")]
    apple_music: bool,
    #[serde(rename = "Purchased")]
    purchased: bool,
    #[serde(rename = "Matched")]
    matched: bool,
    #[serde(rename = "Protected")]
    protected: bool,
    #[serde(rename = "Podcast")]
    podcast: bool,
    #[serde(rename = "Movie")]
    movie: bool,
    #[serde(rename = "TV Show")]
    tv_show: bool,
    #[serde(rename = "Music Video")]
    music_video: bool,
    #[serde(rename = "Audiobooks")]
    audiobooks: bool,
    #[serde(rename = "Has Video")]
    has_video: bool,
}

impl Track {
    fn into_item(self) -> Option<MusicLibraryItem> {
        // Uppercased to match the format the system library reports.
        let persistent_id = self.persistent_id?.to_ascii_uppercase();
        let location = self.location.as_deref().and_then(file_url_to_path);
        let file_present = location.as_ref().is_some_and(|path| path.is_file());
        let ownership = classify(&Evidence {
            kind: &self.kind,
            apple_music: self.apple_music,
            purchased: self.purchased,
            drm_protected: self.protected,
            in_cloud: self.matched || self.track_type.as_deref() == Some("Remote"),
            file_recorded: location.is_some() && self.track_type.as_deref() == Some("File"),
            file_present,
        });
        let media = if self.podcast {
            MediaType::Podcast
        } else if self.audiobooks {
            MediaType::Audiobook
        } else if self.movie || self.tv_show || self.music_video || self.has_video {
            MediaType::Video
        } else {
            MediaType::Music
        };
        Some(MusicLibraryItem {
            persistent_id,
            title: self.name,
            artist: self.artist,
            album_artist: self.album_artist,
            album: self.album,
            genre: self.genre,
            duration_ms: self.total_time,
            track_number: self.track_number,
            disc_number: self.disc_number,
            year: self.year,
            kind: self.kind,
            media,
            ownership,
            location,
            file_present,
            play_count: self.play_count,
            skip_count: self.skip_count,
            rating: self.rating,
            date_added: self.date_added.map(unix_seconds),
            last_played: self.last_played.map(unix_seconds),
            last_skipped: self.last_skipped.map(unix_seconds),
            enabled: !self.disabled,
        })
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Playlist {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Playlist Persistent ID")]
    persistent_id: Option<String>,
    #[serde(rename = "Parent Persistent ID")]
    parent_persistent_id: Option<String>,
    #[serde(rename = "Master")]
    master: bool,
    // Absent means visible.
    #[serde(rename = "Visible")]
    visible: Option<bool>,
    // Present and non-zero on the playlists iTunes itself maintains (Music, Purchased, ...).
    #[serde(rename = "Distinguished Kind")]
    distinguished_kind: u32,
    #[serde(rename = "Folder")]
    folder: bool,
    // Only whether the rules exist matters; their binary format is not read.
    #[serde(rename = "Smart Info")]
    smart_info: Option<IgnoredAny>,
    #[serde(rename = "Smart Criteria")]
    smart_criteria: Option<IgnoredAny>,
    #[serde(rename = "Playlist Items")]
    items: Vec<PlaylistItem>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct PlaylistItem {
    #[serde(rename = "Track ID")]
    track_id: u64,
}

impl Playlist {
    fn into_playlist(self, persistent_ids: &HashMap<u64, String>) -> Option<MusicPlaylist> {
        let system = self.master || self.distinguished_kind != 0 || self.visible == Some(false);
        if system {
            return None;
        }
        let kind = if self.folder {
            PlaylistKind::Folder
        } else if self.smart_info.is_some() || self.smart_criteria.is_some() {
            PlaylistKind::Smart
        } else {
            PlaylistKind::Regular
        };
        Some(MusicPlaylist {
            persistent_id: self.persistent_id?.to_ascii_uppercase(),
            name: self.name,
            parent_persistent_id: self.parent_persistent_id.map(|id| id.to_ascii_uppercase()),
            kind,
            item_ids: self
                .items
                .iter()
                .filter_map(|item| persistent_ids.get(&item.track_id).cloned())
                .collect(),
        })
    }
}

fn unix_seconds(date: plist::Date) -> i64 {
    match SystemTime::from(date).duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    }
}

/// Turns iTunes' `file://localhost/...` URL into a path. Windows drive paths lose the leading
/// slash (`/C:/Music` becomes `C:/Music`) and keep forward slashes, which Windows accepts and
/// `Path` compares as equal to backslashes. Hosts other than `localhost` are not local files.
fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let encoded = rest.strip_prefix("localhost").unwrap_or(rest);
    if !encoded.starts_with('/') {
        return None;
    }
    let decoded = percent_decode_str(encoded).decode_utf8().ok()?;
    let bytes = decoded.as_bytes();
    let has_drive_letter = bytes.len() >= 3 && bytes[1].is_ascii_alphabetic() && bytes[2] == b':';
    let path = if has_drive_letter {
        &decoded[1..]
    } else {
        &decoded
    };
    Some(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_unix_locations() {
        assert_eq!(
            file_url_to_path("file://localhost/Users/riley/Music/Bj%C3%B6rk/01%20Hunter.m4a"),
            Some(PathBuf::from("/Users/riley/Music/Björk/01 Hunter.m4a"))
        );
        assert_eq!(
            file_url_to_path("file:///Users/riley/Music/a%26b.mp3"),
            Some(PathBuf::from("/Users/riley/Music/a&b.mp3"))
        );
    }

    #[test]
    fn decodes_windows_drive_locations() {
        assert_eq!(
            file_url_to_path("file://localhost/C:/Users/riley/My%20Music/song.mp3"),
            Some(PathBuf::from("C:/Users/riley/My Music/song.mp3"))
        );
    }

    #[test]
    fn keeps_unc_locations() {
        assert_eq!(
            file_url_to_path("file://localhost//nas/music/song.mp3"),
            Some(PathBuf::from("//nas/music/song.mp3"))
        );
    }

    #[test]
    fn rejects_non_local_urls() {
        assert_eq!(file_url_to_path("https://example.com/episode.mp3"), None);
        assert_eq!(file_url_to_path("file://nas/music/song.mp3"), None);
        assert_eq!(file_url_to_path("file://localhost/%FF.mp3"), None);
    }
}
