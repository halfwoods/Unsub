/// A service whose export Unsub can read. Last.fm has no file export; its history comes through
/// ListenBrainz, which imports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Service {
    Spotify,
    YoutubeMusic,
    ListenBrainz,
}

impl Service {
    /// The name as people write it, for folders and labels.
    pub fn name(self) -> &'static str {
        match self {
            Self::Spotify => "Spotify",
            Self::YoutubeMusic => "YouTube Music",
            Self::ListenBrainz => "ListenBrainz",
        }
    }

    /// Marks the plays this service gave the library. Importing again dedupes on it, so it must
    /// never change.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Spotify => "spotify",
            Self::YoutubeMusic => "youtube-music",
            Self::ListenBrainz => "listenbrainz",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Song {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    /// The service's own id (a Spotify URI, a YouTube video id), when the export has one.
    pub service_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub name: String,
    pub songs: Vec<Song>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Play {
    /// Unix seconds, UTC. Spotify and Google give the moment the play ended, ListenBrainz when it
    /// began; either way the same export always gives the same value, which is what matters for
    /// importing it twice.
    pub at: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Only Spotify says how long the song was actually heard.
    pub ms_played: Option<u64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Export {
    pub playlists: Vec<Playlist>,
    pub plays: Vec<Play>,
    /// Plays under 30 seconds that were left out, as Spotify doesn't count them as listens either.
    pub short_plays: usize,
    /// Playlist entries that name only a video, which nothing else in the export could name.
    pub unnamed: usize,
}

/// Spotify counts a stream once it has played this long, so a skipped song never becomes a play
/// here either.
pub(crate) const MIN_PLAY_MS: u64 = 30_000;
