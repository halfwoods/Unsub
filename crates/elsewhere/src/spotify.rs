use std::path::PathBuf;

use serde::Deserialize;

use crate::csv::{self, Table};
use crate::error::Result;
use crate::model::{Export, MIN_PLAY_MS, Play, Playlist, Song};
use crate::read::{file_name, json, playlist_name, text};
use crate::time;

/// Both Spotify downloads, and Exportify's CSVs:
///
/// - Account data: `Playlist1.json` (playlists, with no lengths), `YourLibrary.json` (whose saved
///   songs are the "Liked Songs" playlist) and `StreamingHistory_music_0.json` (the last year of
///   plays, to the minute).
/// - Extended streaming history: `Streaming_History_Audio_<years>_0.json`, or `endsong_0.json` in
///   older exports: every play since the account began, to the second, with the album.
/// - Exportify: one `<playlist name>.csv` per playlist, with lengths and Spotify URIs.
pub(crate) fn read(files: &[PathBuf]) -> Result<Export> {
    let mut export = Export::default();
    let (mut account, mut extended) = (Heard::default(), Heard::default());
    for file in files {
        let name = file_name(file);
        let json_file = name.ends_with(".json");
        if json_file && name.starts_with("playlist") {
            for playlist in json::<Playlists>(file)?.playlists {
                export.playlists.push(playlist.into());
            }
        } else if json_file && name == "yourlibrary.json" {
            let liked: Vec<Song> = json::<Library>(file)?
                .tracks
                .into_iter()
                .filter_map(LikedTrack::into_song)
                .collect();
            if !liked.is_empty() {
                export.playlists.push(Playlist {
                    name: "Liked Songs".to_owned(),
                    songs: liked,
                });
            }
        } else if json_file
            && (name.starts_with("streaming_history_audio") || name.starts_with("endsong"))
        {
            extended.add_extended(json(file)?);
        } else if json_file && name.starts_with("streaminghistory") && is_music_history(&name) {
            account.add_account(json(file)?);
        } else if name.ends_with(".csv")
            && let Some(Table::Songs(songs)) = csv::read(&text(file)?)
        {
            export.playlists.push(Playlist {
                name: playlist_name(file),
                songs,
            });
        }
    }
    // The extended history covers the account data's year in more detail, so reading both would
    // count that year twice.
    let heard = if extended.plays.is_empty() {
        account
    } else {
        extended
    };
    export.plays = heard.plays;
    export.short_plays = heard.short;
    Ok(export)
}

/// `StreamingHistory0.json` from before 2023, or `StreamingHistory_music_0.json`; the podcast and
/// video files beside it are not songs.
fn is_music_history(name: &str) -> bool {
    let rest = name.trim_start_matches("streaminghistory");
    rest.contains("music") || rest.starts_with(|c: char| c.is_ascii_digit())
}

#[derive(Default)]
struct Heard {
    plays: Vec<Play>,
    short: usize,
}

impl Heard {
    fn add_account(&mut self, rows: Vec<AccountPlay>) {
        for row in rows {
            let Some(at) = time::unix(&row.end_time) else {
                continue;
            };
            if row.track_name.is_empty() || row.artist_name.is_empty() {
                continue;
            }
            if row.ms_played < MIN_PLAY_MS {
                self.short += 1;
                continue;
            }
            self.plays.push(Play {
                at,
                title: row.track_name,
                artist: row.artist_name,
                album: None,
                ms_played: Some(row.ms_played),
            });
        }
    }

    fn add_extended(&mut self, rows: Vec<ExtendedPlay>) {
        for row in rows {
            // Podcasts, audiobooks and videos have no track name.
            let (Some(title), Some(artist), Some(at)) = (
                row.master_metadata_track_name,
                row.master_metadata_album_artist_name,
                time::unix(&row.ts),
            ) else {
                continue;
            };
            if row.ms_played < MIN_PLAY_MS {
                self.short += 1;
                continue;
            }
            // The account data's plays are only to the minute. Filing these under their minute too
            // makes a play in both exports the same play, if someone imports both.
            let at = at - at.rem_euclid(60);
            self.plays.push(Play {
                at,
                title,
                artist,
                album: row.master_metadata_album_album_name,
                ms_played: Some(row.ms_played),
            });
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountPlay {
    #[serde(default)]
    end_time: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    ms_played: u64,
}

/// Only what is read: the file also holds the IP address and device of every play, which stay
/// unread.
#[derive(Deserialize)]
struct ExtendedPlay {
    #[serde(default)]
    ts: String,
    #[serde(default)]
    ms_played: u64,
    master_metadata_track_name: Option<String>,
    master_metadata_album_artist_name: Option<String>,
    master_metadata_album_album_name: Option<String>,
}

#[derive(Deserialize, Default)]
struct Playlists {
    #[serde(default)]
    playlists: Vec<SpotifyPlaylist>,
}

#[derive(Deserialize)]
struct SpotifyPlaylist {
    #[serde(default)]
    name: String,
    #[serde(default)]
    items: Vec<Item>,
}

/// Episodes have no `track`. A local file (one the person added from their own disk) has only a
/// `localTrack` holding a `spotify:local:` URI.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    track: Option<SpotifyTrack>,
    local_track: Option<LocalTrack>,
}

#[derive(Deserialize)]
struct LocalTrack {
    uri: Option<String>,
}

impl LocalTrack {
    /// `spotify:local:<artist>:<album>:<title>:<seconds>`, form-encoded ("+" for a space). A file
    /// with no tags has no artist or title to go on, and is left out.
    fn into_song(self) -> Option<Song> {
        let uri = self.uri?;
        let mut fields = uri.strip_prefix("spotify:local:")?.split(':').map(decode);
        let (artist, album, title, seconds) = (
            fields.next()?,
            fields.next()?,
            fields.next()?,
            fields.next()?,
        );
        let seconds = seconds.trim().parse::<u64>().ok();
        Some(Song {
            title: Some(title.trim().to_owned()).filter(|title| !title.is_empty())?,
            artist: Some(artist.trim().to_owned()).filter(|artist| !artist.is_empty())?,
            album: Some(album.trim().to_owned()).filter(|album| !album.is_empty()),
            duration_ms: seconds.map(|seconds| seconds * 1_000).filter(|ms| *ms > 0),
            service_id: Some(uri),
        })
    }
}

/// Undoes form encoding: "+" is a space and "%XX" is a byte of UTF-8.
fn decode(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok());
        match (
            bytes[at],
            hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()),
        ) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                at += 3;
            }
            (b'+', _) => {
                decoded.push(b' ');
                at += 1;
            }
            (byte, _) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpotifyTrack {
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    track_uri: Option<String>,
}

#[derive(Deserialize, Default)]
struct Library {
    #[serde(default)]
    tracks: Vec<LikedTrack>,
}

/// A saved song in `YourLibrary.json`. The file also lists saved albums and followed artists.
#[derive(Deserialize)]
struct LikedTrack {
    track: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    uri: Option<String>,
}

impl LikedTrack {
    fn into_song(self) -> Option<Song> {
        Some(Song {
            title: self.track.filter(|title| !title.trim().is_empty())?,
            artist: self.artist.filter(|artist| !artist.trim().is_empty())?,
            album: self.album.filter(|album| !album.trim().is_empty()),
            duration_ms: None,
            service_id: self.uri,
        })
    }
}

impl From<SpotifyPlaylist> for Playlist {
    fn from(playlist: SpotifyPlaylist) -> Self {
        let songs = playlist
            .items
            .into_iter()
            .filter_map(|item| {
                let Some(track) = item.track else {
                    return item.local_track?.into_song();
                };
                Some(Song {
                    title: track.track_name.filter(|title| !title.trim().is_empty())?,
                    artist: track
                        .artist_name
                        .filter(|artist| !artist.trim().is_empty())?,
                    album: track.album_name.filter(|album| !album.trim().is_empty()),
                    duration_ms: None,
                    service_id: track.track_uri,
                })
            })
            .collect();
        let name = playlist.name.trim();
        Playlist {
            name: if name.is_empty() {
                "Untitled Playlist".to_owned()
            } else {
                name.to_owned()
            },
            songs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding_is_undone() {
        assert_eq!(decode("Daft+Punk"), "Daft Punk");
        assert_eq!(decode("Caf%C3%A9%2C+100%"), "Café, 100%");
        assert_eq!(decode("50%+off%zz"), "50% off%zz");
    }

    #[test]
    fn a_local_file_needs_an_artist_and_a_title() {
        let local = |uri: &str| {
            LocalTrack {
                uri: Some(uri.to_owned()),
            }
            .into_song()
        };

        assert_eq!(local("spotify:local:::Untitled+Demo:0"), None);
        assert_eq!(local("spotify:local:Band:Album::200"), None);
        assert_eq!(local("spotify:local:a:b"), None);
        assert_eq!(local("spotify:track:abc"), None);
        let song = local("spotify:local:Band::Song:abc").unwrap();
        assert_eq!((song.album, song.duration_ms), (None, None));
    }
}
