use std::path::PathBuf;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

use crate::loudness::TrackLoudness;
use crate::smart::SmartRules;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Music,
    Podcast,
    Audiobook,
    /// An internet radio station: `path` is the stream's URL, and there is no
    /// file to read, move, check or measure.
    Radio,
}

/// The `kind` column's value for stations, for the SQL that skips them.
pub(crate) const RADIO: i64 = MediaKind::Radio as i64;

impl MediaKind {
    /// The names smart playlist rules use, in any case.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        [Self::Music, Self::Podcast, Self::Audiobook, Self::Radio]
            .into_iter()
            .find(|kind| kind.name().eq_ignore_ascii_case(name.trim()))
    }

    fn name(self) -> &'static str {
        match self {
            Self::Music => "music",
            Self::Podcast => "podcast",
            Self::Audiobook => "audiobook",
            Self::Radio => "radio",
        }
    }
}

impl ToSql for MediaKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(*self as i64))
    }
}

impl FromSql for MediaKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_i64()? {
            0 => Ok(Self::Music),
            1 => Ok(Self::Podcast),
            2 => Ok(Self::Audiobook),
            3 => Ok(Self::Radio),
            other => Err(FromSqlError::OutOfRange(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    Name,
    Time,
    Artist,
    Album,
    AlbumArtist,
    Composer,
    Genre,
    Year,
    TrackNumber,
    DiscNumber,
    Rating,
    Plays,
    SkipCount,
    LastPlayed,
    DateAdded,
    Kind,
    Bitrate,
    SampleRate,
    Size,
    /// Position within the playlist being listed; elsewhere it falls back to Artist.
    PlaylistOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackQuery {
    /// `None` lists every kind of file. Stations are only listed when asked
    /// for (`Some(Radio)`), or by a playlist that holds them.
    pub kind: Option<MediaKind>,
    pub search: Option<String>,
    pub sort: SortKey,
    pub descending: bool,
    /// Restricts the list to these track ids, e.g. a device playlist's members.
    pub ids: Option<Vec<i64>>,
    /// Restricts the list to one playlist's contents. Combines with the other filters.
    pub playlist: Option<i64>,
    /// Opaque grouping keys returned by artist and album summaries, not search text.
    pub artist_key: Option<String>,
    pub album_key: Option<String>,
    /// Exact, case/diacritic-insensitive browser filters. Combine with all other scope.
    pub genre: Option<String>,
    pub composer: Option<String>,
}

impl Default for TrackQuery {
    fn default() -> Self {
        Self {
            kind: Some(MediaKind::Music),
            search: None,
            sort: SortKey::Artist,
            descending: false,
            ids: None,
            playlist: None,
            artist_key: None,
            album_key: None,
            genre: None,
            composer: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    /// The album this song is filed under, which an album's `key` in [`AlbumSummary`] is too: the
    /// library's own identity for it, coarser than the tags (accents and case fold, compilations pool).
    /// Empty on songs that don't come from the library, such as a CD's, an iPod's or a shared library's.
    pub album_group: String,
    pub genre: String,
    pub composer: String,
    pub year: Option<i32>,
    pub track_number: Option<u32>,
    pub track_count: Option<u32>,
    pub disc_number: Option<u32>,
    pub disc_count: Option<u32>,
    pub duration_ms: u64,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub codec: String,
    pub file_size: u64,
    pub date_added: i64,
    pub date_modified: i64,
    pub play_count: u32,
    pub last_played: Option<i64>,
    pub skip_count: u32,
    pub rating: u8,
    pub kind: MediaKind,
    pub compilation: bool,
    /// The song checkbox: unchecked songs are skipped by playback and by "only checked" smart playlists.
    pub enabled: bool,
    /// The file is only a placeholder in the cloud (iCloud Drive's "Optimize Mac
    /// Storage"). Its details come from the file's name and folders until it is
    /// downloaded and the scan or `refresh_cloud_status` reads its tags.
    pub in_cloud: bool,
    /// The file was not found at `path` when last checked (`check_missing`); a
    /// rescan that finds it clears the flag, `relink` points the track elsewhere.
    pub missing: bool,
    /// How loud the track is, once known: read from its tags at scan time or
    /// measured by the app.
    pub loudness: Option<TrackLoudness>,
    /// "Remember playback position", as it applies to this track: on for
    /// audiobooks and podcasts unless Get Info says otherwise. Not written by
    /// `import_tracks`, which has no such setting to bring.
    pub remember_position: bool,
    /// The file's own tags carry lyrics, plain or timed, or they were set in
    /// Get Info. A `.lrc` sidecar is not counted: it can appear without the
    /// audio file changing, which is what a scan looks for; `lyrics` finds it.
    pub has_lyrics: bool,
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub count: u64,
    pub total_duration_ms: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artwork {
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub scanned: u64,
    pub added: u64,
    pub current_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub scanned: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub failed: Vec<(String, String)>,
}

/// A song from another library (e.g. Apple Music) to look for in ours.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalTrack {
    pub external_id: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub duration_ms: Option<u64>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub path: Option<String>,
    /// For a song known only from how long it was played (a stream's play log): it is at least
    /// this long, so a shorter track, give or take a skipped bit, is not it.
    #[serde(default)]
    pub heard_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchQuality {
    SameFile,
    Exact,
    Close,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMatch {
    pub external_id: String,
    pub track_id: i64,
    pub quality: MatchQuality,
}

/// Whether an external song is in the library, for a caller that must not guess.
#[derive(Debug, Clone)]
pub enum MatchOutcome {
    Found(TrackMatch),
    /// Tracks on different albums fit equally well, so none is picked.
    Ambiguous,
    Missing,
}

impl MatchOutcome {
    pub fn track_id(&self) -> Option<i64> {
        match self {
            Self::Found(found) => Some(found.track_id),
            Self::Ambiguous | Self::Missing => None,
        }
    }

    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Self::Ambiguous)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistKind {
    Regular,
    Smart,
    Folder,
}

impl ToSql for PlaylistKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(*self as i64))
    }
}

impl FromSql for PlaylistKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_i64()? {
            0 => Ok(Self::Regular),
            1 => Ok(Self::Smart),
            2 => Ok(Self::Folder),
            other => Err(FromSqlError::OutOfRange(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub kind: PlaylistKind,
    pub parent_id: Option<i64>,
    /// Index among its siblings.
    pub position: u32,
    pub smart: Option<SmartRules>,
    /// Rows a listing would show: duplicates count, folders count their subtree once.
    pub track_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    /// Stable for as long as the album's artist and name tags are.
    pub key: String,
    pub album: String,
    pub album_artist: String,
    pub year: Option<i32>,
    pub genre: String,
    pub track_count: u32,
    pub duration_ms: u64,
    /// The album's first-added track; the one to ask `artwork()` about.
    pub artwork_track_id: i64,
    pub date_added: i64,
    pub compilation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistSummary {
    pub key: String,
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
    /// Up to four distinct albums within this query's scope, for local cover mosaics.
    pub artwork_track_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenreSummary {
    pub key: String,
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerSummary {
    pub name: String,
    pub track_count: u32,
}

/// Play data from another library for a track already in ours.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalStats {
    pub play_count: u32,
    pub skip_count: u32,
    pub last_played: Option<i64>,
    pub last_skipped: Option<i64>,
    /// 0..=100, stored as given (capped at 100); the importer should round half stars.
    pub rating: u8,
    pub date_added: Option<i64>,
    pub enabled: bool,
}

/// Metadata that wins over what a file's tags say. Stored with the track, so a
/// rescan re-applies it instead of reverting to the tags.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackOverrides {
    pub kind: Option<MediaKind>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub date_added: Option<i64>,
    pub year: Option<i32>,
    pub composer: Option<String>,
    pub track_number: Option<u32>,
    pub track_count: Option<u32>,
    pub disc_number: Option<u32>,
    pub disc_count: Option<u32>,
    pub compilation: Option<bool>,
    /// Plain lyrics, for a file that cannot carry them; empty means none.
    pub lyrics: Option<String>,
    /// For a placeholder, whose length can't be read; a file's own length is never replaced.
    pub duration_ms: Option<u64>,
}

impl TrackOverrides {
    /// These, with `theirs` added for each detail neither these nor `track` already hold, so
    /// importing again never replaces what the library knows or the user has changed. `None`
    /// for a track whose own details are only guesses from its file name: then only these count.
    pub(crate) fn filled_from(mut self, theirs: &Self, track: Option<&Track>) -> Self {
        let text = |own: &mut Option<String>, new: &Option<String>, held: fn(&Track) -> &str| {
            if own.is_none() && track.is_none_or(|track| held(track).is_empty()) {
                own.clone_from(new);
            }
        };
        text(&mut self.title, &theirs.title, |t| &t.title);
        text(&mut self.artist, &theirs.artist, |t| &t.artist);
        text(&mut self.album, &theirs.album, |t| &t.album);
        text(&mut self.album_artist, &theirs.album_artist, |t| {
            &t.album_artist
        });
        text(&mut self.genre, &theirs.genre, |t| &t.genre);
        text(&mut self.composer, &theirs.composer, |t| &t.composer);
        let number = |own: &mut Option<u32>, new: Option<u32>, held: fn(&Track) -> Option<u32>| {
            if own.is_none() && track.is_none_or(|track| held(track).is_none()) {
                *own = new;
            }
        };
        number(&mut self.track_number, theirs.track_number, |t| {
            t.track_number
        });
        number(&mut self.track_count, theirs.track_count, |t| t.track_count);
        number(&mut self.disc_number, theirs.disc_number, |t| t.disc_number);
        number(&mut self.disc_count, theirs.disc_count, |t| t.disc_count);
        if self.year.is_none() && track.is_none_or(|track| track.year.is_none()) {
            self.year = theirs.year;
        }
        if self.duration_ms.is_none() && track.is_none_or(|track| track.duration_ms == 0) {
            self.duration_ms = theirs.duration_ms;
        }
        self.kind = self.kind.or(theirs.kind);
        self.date_added = self.date_added.or(theirs.date_added);
        self.compilation = self.compilation.or(theirs.compilation);
        self.lyrics = self.lyrics.or_else(|| theirs.lyrics.clone());
        self
    }
}

/// A file for `copy_files_in`: the details its song is to have, and what the caller knows it by.
#[derive(Debug, Clone)]
pub struct CopyIn {
    pub path: PathBuf,
    pub overrides: TrackOverrides,
    /// Recorded with the song in the same transaction, so `rescued` can say later that this one is
    /// already in the library.
    pub source_key: String,
}

/// What `add_files` made of its input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddSummary {
    /// The library id each input file became, in input order, whether it was
    /// new or already in the library; `None` for one that failed.
    pub ids: Vec<Option<i64>>,
    /// The files that could not be added, with why.
    pub failed: Vec<(String, String)>,
}

/// A themed, generated playlist. The key is stable for as long as the cluster it
/// came from exists, so the UI can remember which mix is selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mix {
    pub key: String,
    pub name: String,
    pub track_ids: Vec<i64>,
    /// Up to four tracks from distinct albums, for a collage.
    pub artwork_track_ids: Vec<i64>,
}
