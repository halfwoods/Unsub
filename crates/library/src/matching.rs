use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::OptionalExtension;

use crate::error::Result;
use crate::library::Library;
use crate::model::{ExternalTrack, MatchOutcome, MatchQuality, RADIO, TrackMatch};
use crate::normalize::{GENERIC_TITLES, artist_key, title_key};
use crate::tags;

const EXACT_TOLERANCE_MS: u64 = 3_000;
const CLOSE_TOLERANCE_MS: u64 = 8_000;
/// How much shorter than the time it was played a track may be: a bit skipped, or a gap.
const HEARD_SLACK_MS: u64 = 10_000;
/// Match keys of one external track, computed once.
struct Wanted {
    title: Option<String>,
    /// Both artist fields, since taggers disagree on which one holds the artist.
    artists: Vec<String>,
    album: Option<String>,
    duration_ms: Option<u64>,
    heard_ms: Option<u64>,
    /// Title and artist alone say little here, and the caller wants a sure match.
    needs_evidence: bool,
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

/// The best candidate, and whether another track on a different album was just as good a fit.
struct Best {
    track_id: i64,
    quality: MatchQuality,
    ambiguous: bool,
}

/// What looking for one song found.
enum Verdict {
    /// A track, and whether another on a different album fit as well.
    Match(TrackMatch, bool),
    /// Tracks with this title and artist exist, but nothing says one of them is this song.
    Unsure,
    Nothing,
}

/// How one track with the right title and artist compares.
enum Fit {
    Fits(Rank, MatchQuality),
    /// It may be the song, but there is nothing to say so: a generic title with no album or
    /// length to go on, or a track shorter than the song was played.
    Unsure,
    /// It is another recording.
    Different,
}

/// What `best_candidate` made of a song's candidates.
enum Search {
    Found(Best),
    Unsure,
    Nothing,
}

impl ExternalTrack {
    /// A song described by its own file's tags, for a source that has no database to say what its
    /// files are.
    pub fn from_file(external_id: impl Into<String>, path: &Path) -> Result<Self> {
        let meta = tags::read(path)?;
        Ok(Self {
            external_id: external_id.into(),
            title: meta.title,
            artist: meta.artist,
            album_artist: meta.album_artist,
            album: meta.album,
            duration_ms: Some(meta.duration_ms),
            track_number: meta.track_number,
            disc_number: meta.disc_number,
            path: None,
            heard_ms: None,
        })
    }
}

impl Library {
    /// One result per input, in input order, against every kind of file in the library.
    /// A false "you own this" costs more than a miss, so anything ambiguous in
    /// title, artist or duration is left unmatched.
    pub fn match_external(&self, tracks: &[ExternalTrack]) -> Result<Vec<Option<TrackMatch>>> {
        Ok(self
            .find(tracks, false)?
            .into_iter()
            .map(|verdict| match verdict {
                Verdict::Match(found, _) => Some(found),
                Verdict::Unsure | Verdict::Nothing => None,
            })
            .collect())
    }

    /// Like `match_external`, but for a caller that must not guess between
    /// releases: a song that fits tracks on different albums equally well
    /// (nothing says which album it is from) is `Ambiguous`, not given the
    /// lowest id's. So is one whose only candidates have nothing to say they
    /// are it (a very common title with no album or length to go on, a track
    /// shorter than the song was played): the song is not known to be missing.
    /// Copies of one album's track are the same song and still match.
    pub fn match_external_strict(&self, tracks: &[ExternalTrack]) -> Result<Vec<MatchOutcome>> {
        Ok(self
            .find(tracks, true)?
            .into_iter()
            .map(|verdict| match verdict {
                Verdict::Match(_, true) | Verdict::Unsure => MatchOutcome::Ambiguous,
                Verdict::Match(found, false) => MatchOutcome::Found(found),
                Verdict::Nothing => MatchOutcome::Missing,
            })
            .collect())
    }

    fn find(&self, tracks: &[ExternalTrack], strict: bool) -> Result<Vec<Verdict>> {
        let wanted: Vec<Wanted> = tracks.iter().map(|t| wanted_keys(t, strict)).collect();
        let buckets = self.candidates(&wanted)?;
        tracks
            .iter()
            .zip(&wanted)
            .map(|(external, wanted)| {
                let search = match self.same_file(external)? {
                    Some(track_id) => Search::Found(Best {
                        track_id,
                        quality: MatchQuality::SameFile,
                        ambiguous: false,
                    }),
                    None => best_candidate(wanted, &buckets),
                };
                Ok(match search {
                    Search::Found(best) => Verdict::Match(
                        TrackMatch {
                            external_id: external.external_id.clone(),
                            track_id: best.track_id,
                            quality: best.quality,
                        },
                        best.ambiguous,
                    ),
                    Search::Unsure => Verdict::Unsure,
                    Search::Nothing => Verdict::Nothing,
                })
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

        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, title, artist, album_artist, album, duration_ms FROM tracks
                 WHERE kind <> {RADIO}"
        ))?;
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

fn wanted_keys(external: &ExternalTrack, strict: bool) -> Wanted {
    let first = artist_key(&external.artist);
    let second = artist_key(&external.album_artist).filter(|key| Some(key) != first.as_ref());
    let title = title_key(&external.title);
    Wanted {
        needs_evidence: strict
            && title
                .as_deref()
                .is_some_and(|title| GENERIC_TITLES.contains(&title)),
        title,
        artists: [first, second].into_iter().flatten().collect(),
        album: title_key(&external.album),
        duration_ms: known_duration(external.duration_ms),
        heard_ms: known_duration(external.heard_ms),
    }
}

/// A zero duration is what unreadable files report, not a real length.
fn known_duration(duration_ms: Option<u64>) -> Option<u64> {
    duration_ms.filter(|ms| *ms > 0)
}

fn best_candidate(wanted: &Wanted, buckets: &Buckets) -> Search {
    let Some(by_artist) = wanted.title.as_ref().and_then(|title| buckets.get(title)) else {
        return Search::Nothing;
    };
    let candidates = || {
        wanted
            .artists
            .iter()
            .filter_map(|artist| by_artist.get(artist))
            .flatten()
            .map(|candidate| (classify(wanted, candidate), &candidate.album))
    };
    let fits = || {
        candidates().filter_map(|(fit, album)| match fit {
            Fit::Fits(rank, quality) => Some((rank, quality, album)),
            Fit::Unsure | Fit::Different => None,
        })
    };
    match fits().min_by_key(|(rank, ..)| *rank) {
        Some((best, quality, album)) => Search::Found(Best {
            track_id: best.3,
            quality,
            ambiguous: fits()
                .any(|(rank, _, other)| rank.0 == best.0 && rank.1 == best.1 && other != album),
        }),
        None if candidates().any(|(fit, _)| matches!(fit, Fit::Unsure)) => Search::Unsure,
        None => Search::Nothing,
    }
}

/// Title and artist already agree; this decides whether the durations (or,
/// failing those, the album) make it the same recording.
fn classify(wanted: &Wanted, candidate: &Candidate) -> Fit {
    if let Some((heard, actual)) = wanted.heard_ms.zip(candidate.duration_ms)
        && actual + HEARD_SLACK_MS < heard
    {
        return Fit::Unsure;
    }
    let same_album = wanted.album.is_some() && wanted.album == candidate.album;
    let gap = wanted
        .duration_ms
        .zip(candidate.duration_ms)
        .map(|(a, b)| a.abs_diff(b));
    let quality = match gap {
        Some(gap) if gap <= EXACT_TOLERANCE_MS => MatchQuality::Exact,
        Some(gap) if gap <= CLOSE_TOLERANCE_MS => MatchQuality::Close,
        Some(_) => return Fit::Different,
        None if same_album => MatchQuality::Exact,
        None if wanted.needs_evidence => return Fit::Unsure,
        None => MatchQuality::Close,
    };
    let rank = (
        quality as u8,
        !same_album,
        gap.unwrap_or(u64::MAX),
        candidate.id,
    );
    Fit::Fits(rank, quality)
}
