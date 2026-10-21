use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde::de::{SeqAccess, Visitor};

use crate::csv::{self, Table};
use crate::error::Result;
use crate::model::{Export, Play, Playlist, Song};
use crate::read::{file_name, json, playlist_name, text};
use crate::time;

/// Google Takeout, "YouTube and YouTube Music":
///
/// - `history/watch-history.json`: every video watched, music included. The music entries carry
///   the song and artist, and are what plays are made from.
/// - `playlists/<name> videos.csv`: a playlist, as video ids and the time each was added, with no
///   names. They are named from the other files that know the same video.
/// - `music (library and uploads)/music library songs.csv`: saved songs, with album and artists.
///   Read as a playlist of its own. Uploads are the person's own files, so they are skipped.
pub(crate) fn read(files: &[PathBuf]) -> Result<Export> {
    let mut export = Export::default();
    let mut known: HashMap<String, Song> = HashMap::new();
    let mut by_id: Vec<(String, Vec<String>)> = Vec::new();
    for file in files {
        let name = file_name(file);
        if name.starts_with("watch-history") && name.ends_with(".json") {
            for activity in json::<MusicHistory>(file)?.0 {
                let Some((play, video)) = activity.into_play() else {
                    continue;
                };
                if let Some(video) = video {
                    known.entry(video).or_insert_with(|| Song {
                        title: play.title.clone(),
                        artist: play.artist.clone(),
                        album: None,
                        duration_ms: None,
                        service_id: None,
                    });
                }
                export.plays.push(play);
            }
        } else if name.ends_with(".csv") && !name.contains("upload") {
            match csv::read(&text(file)?) {
                Some(Table::Songs(songs)) => {
                    for song in &songs {
                        if let Some(video) = &song.service_id {
                            known.insert(video.clone(), song.clone());
                        }
                    }
                    export.playlists.push(Playlist {
                        name: playlist_name(file),
                        songs,
                    });
                }
                Some(Table::VideoIds(videos)) => by_id.push((playlist_name(file), videos)),
                None => {}
            }
        }
    }
    for (name, videos) in by_id {
        let songs: Vec<Song> = videos
            .iter()
            .filter_map(|video| known.get(video).cloned())
            .collect();
        export.unnamed += videos.len() - songs.len();
        export.playlists.push(Playlist { name, songs });
    }
    Ok(export)
}

const MUSIC_HEADER: &str = "YouTube Music";
/// YouTube Music's own artist channels are all called "<artist> - Topic".
const TOPIC_SUFFIX: &str = " - Topic";
const ADVERTISEMENT: &str = "From Google Ads";
const WATCHED: &str = "Watched ";

/// One entry of Google's "My Activity" format.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Activity {
    #[serde(default)]
    header: String,
    #[serde(default)]
    title: String,
    title_url: Option<String>,
    /// The channel: the artist, for a song.
    subtitles: Option<Vec<Named>>,
    time: Option<String>,
    details: Option<Vec<Named>>,
}

#[derive(Deserialize)]
struct Named {
    name: Option<String>,
}

impl Activity {
    fn is_music(&self) -> bool {
        self.header.eq_ignore_ascii_case(MUSIC_HEADER)
            || self
                .title_url
                .as_deref()
                .is_some_and(|url| url.contains("music.youtube.com"))
    }

    /// The play, and the video it was of. Only songs YouTube Music itself serves count: an artist's
    /// auto-generated "<artist> - Topic" channel, under a title that begins "Watched". A music
    /// video, an upload or a removed video has no artist and song to read out of it, and would
    /// only put junk on the To Own list.
    fn into_play(self) -> Option<(Play, Option<String>)> {
        if self
            .details
            .iter()
            .flatten()
            .any(|detail| detail.name.as_deref() == Some(ADVERTISEMENT))
        {
            return None;
        }
        let channel = self.subtitles?.into_iter().next()?.name?;
        let artist = channel.strip_suffix(TOPIC_SUFFIX)?.trim().to_owned();
        // Google writes this verb in the account's language; only the English one is known.
        let title = self.title.strip_prefix(WATCHED)?.trim().to_owned();
        if title.is_empty() || artist.is_empty() {
            return None;
        }
        let video = self.title_url.as_deref().and_then(video_id);
        Some((
            Play {
                at: time::unix(self.time.as_deref()?)?,
                title,
                artist,
                album: None,
                ms_played: None,
            },
            video,
        ))
    }
}

fn video_id(url: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("v="))
        .map(str::to_owned)
}

/// The music entries of a watch history. The array is walked entry by entry because most of it is
/// ordinary videos, and a long history is hundreds of megabytes.
struct MusicHistory(Vec<Activity>);

impl<'de> Deserialize<'de> for MusicHistory {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Walk;
        impl<'de> Visitor<'de> for Walk {
            type Value = MusicHistory;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a list of activity")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<MusicHistory, A::Error> {
                let mut music = Vec::new();
                while let Some(activity) = seq.next_element::<Activity>()? {
                    if activity.is_music() {
                        music.push(activity);
                    }
                }
                Ok(MusicHistory(music))
            }
        }
        deserializer.deserialize_seq(Walk)
    }
}
