use std::collections::HashMap;
use std::fs;

use rusqlite::OptionalExtension;

use crate::error::Result;
use crate::library::Library;
use crate::model::{ExternalTrack, MatchQuality, TrackMatch};
use crate::normalize::{artist_key, title_key};

const EXACT_TOLERANCE_MS: u64 = 3_000;
const CLOSE_TOLERANCE_MS: u64 = 8_000;

/// Match keys of one external track, computed once.
struct Wanted {
    title: Option<String>,
    /// Both artist fields, since taggers disagree on which one holds the artist.
    artists: Vec<String>,
    album: Option<String>,
    duration_ms: Option<u64>,
}

struct Candidate {
    id: i64,
    album: Option<String>,
    duration_ms: Option<u64>,
}

/// Library tracks that share a (title key, artist key) with some external
/// track. Nested so lookups borrow `&str` instead of building tuple keys.
type Buckets = HashMap<String, HashMap<String, Vec<Candidate>>>;

/// Sorts best-first: quality tier, same album, closest duration, lowest id.
type Rank = (u8, bool, u64, i64);

impl Library {
    /// One result per input, in input order, against every kind in the library.
    /// A false "you own this" costs more than a miss, so anything ambiguous in
    /// title, artist or duration is left unmatched.
    pub fn match_external(&self, tracks: &[ExternalTrack]) -> Result<Vec<Option<TrackMatch>>> {
        let wanted: Vec<Wanted> = tracks.iter().map(wanted_keys).collect();
        let buckets = self.candidates(&wanted)?;
        tracks
            .iter()
            .zip(&wanted)
            .map(|(external, wanted)| {
                let found = match self.same_file(external)? {
                    Some(id) => Some((id, MatchQuality::SameFile)),
                    None => best_candidate(wanted, &buckets),
                };
                Ok(found.map(|(track_id, quality)| TrackMatch {
                    external_id: external.external_id.clone(),
                    track_id,
                    quality,
                }))
            })
            .collect()
    }

    /// Streams the library once, keying only rows whose title is wanted so the
    /// artist and album work is skipped for the rest.
    fn candidates(&self, wanted: &[Wanted]) -> Result<Buckets> {
        let mut buckets = Buckets::new();
        for (title, artists) in wanted
            .iter()
            .filter_map(|w| Some((w.title.as_ref()?, &w.artists)))
        {
            let by_artist = buckets.entry(title.clone()).or_default();
            for artist in artists {
                by_artist.entry(artist.clone()).or_default();
            }
        }

        let mut stmt = self
            .conn
            .prepare("SELECT id, title, artist, album_artist, album, duration_ms FROM tracks")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let Some(by_artist) =
                title_key(&row.get::<_, String>(1)?).and_then(|key| buckets.get_mut(&key))
            else {
                continue;
            };
            let first = artist_key(&row.get::<_, String>(2)?);
            let second =
                artist_key(&row.get::<_, String>(3)?).filter(|key| Some(key) != first.as_ref());
            for artist in [first, second].into_iter().flatten() {
                if let Some(bucket) = by_artist.get_mut(&artist) {
                    bucket.push(Candidate {
                        id: row.get(0)?,
                        album: title_key(&row.get::<_, String>(4)?),
                        duration_ms: known_duration(Some(row.get(5)?)),
                    });
                }
            }
        }
        Ok(buckets)
    }

    /// Tries the path as given first, which avoids a filesystem call in the
    /// common case of stored paths that were canonical to begin with.
    fn same_file(&self, external: &ExternalTrack) -> Result<Option<i64>> {
        let Some(path) = &external.path else {
            return Ok(None);
        };
        if let Some(id) = self.track_id_at(path)? {
            return Ok(Some(id));
        }
        match fs::canonicalize(path)
            .ok()
            .as_deref()
            .and_then(|p| p.to_str())
        {
            Some(canonical) if canonical != path => self.track_id_at(canonical),
            _ => Ok(None),
        }
    }

    fn track_id_at(&self, path: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .prepare_cached("SELECT id FROM tracks WHERE path = ?1")?
            .query_row([path], |row| row.get(0))
            .optional()?)
    }
}

fn wanted_keys(external: &ExternalTrack) -> Wanted {
    let first = artist_key(&external.artist);
    let second = artist_key(&external.album_artist).filter(|key| Some(key) != first.as_ref());
    Wanted {
        title: title_key(&external.title),
        artists: [first, second].into_iter().flatten().collect(),
        album: title_key(&external.album),
        duration_ms: known_duration(external.duration_ms),
    }
}

/// A zero duration is what unreadable files report, not a real length.
fn known_duration(duration_ms: Option<u64>) -> Option<u64> {
    duration_ms.filter(|ms| *ms > 0)
}

fn best_candidate(wanted: &Wanted, buckets: &Buckets) -> Option<(i64, MatchQuality)> {
    let by_artist = buckets.get(wanted.title.as_ref()?)?;
    wanted
        .artists
        .iter()
        .filter_map(|artist| by_artist.get(artist))
        .flatten()
        .filter_map(|candidate| classify(wanted, candidate))
        .min_by_key(|(rank, _)| *rank)
        .map(|(rank, quality)| (rank.3, quality))
}

/// Title and artist already agree; this decides whether the durations (or,
/// failing those, the album) make it the same recording.
fn classify(wanted: &Wanted, candidate: &Candidate) -> Option<(Rank, MatchQuality)> {
    let same_album = wanted.album.is_some() && wanted.album == candidate.album;
    let gap = wanted
        .duration_ms
        .zip(candidate.duration_ms)
        .map(|(a, b)| a.abs_diff(b));
    let quality = match gap {
        Some(gap) if gap <= EXACT_TOLERANCE_MS => MatchQuality::Exact,
        Some(gap) if gap <= CLOSE_TOLERANCE_MS => MatchQuality::Close,
        Some(_) => return None,
        None if same_album => MatchQuality::Exact,
        None => MatchQuality::Close,
    };
    let rank = (
        quality as u8,
        !same_album,
        gap.unwrap_or(u64::MAX),
        candidate.id,
    );
    Some((rank, quality))
}
