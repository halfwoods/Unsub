use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Row};

use crate::error::Result;
use crate::library::{Library, PLAY_EVENT};
use crate::model::{MediaKind, Mix};
use crate::normalize::{COMPILATIONS_GROUP, artist_names};

/// What each signal is worth when ranking songs against the seeds; only the
/// ratios matter, so tune them against each other. Behaviour (played together,
/// listed together) is worth more than tags, and tags more than anything else.
/// Year and rating never link songs: they only order songs that already have
/// a link, so nothing qualifies on rating alone.
struct Weights {
    co_play: f64,
    playlist: f64,
    same_artist: f64,
    co_artist: f64,
    genre: f64,
    year: f64,
    rating: f64,
}

const WEIGHTS: Weights = Weights {
    co_play: 6.0,
    playlist: 4.0,
    same_artist: 5.0,
    co_artist: 3.0,
    genre: 4.0,
    year: 2.0,
    rating: 1.5,
};

/// Plays at least this far apart belong to different listening sessions.
const SESSION_GAP_SECONDS: i64 = 30 * 60;
/// How many plays either side of a song still count as played together.
const CO_PLAY_WINDOW: usize = 10;
/// Years apart at which two songs stop counting as contemporaries.
const YEAR_SPAN: f64 = 20.0;
/// Joint credits beyond this many songs add no further affinity.
const CO_ARTIST_CAP: u32 = 5;
const DIVERSITY_WINDOW: usize = 10;
const MAX_PER_ARTIST_IN_WINDOW: usize = 2;
const MIX_ARTWORK: usize = 4;
/// How many of a cluster's best songs stand for it when it is a mix's profile.
const CLUSTER_SEEDS: usize = 5;
/// An artist needs this many songs to anchor a mix of their own.
const ARTIST_MIX_MIN_TRACKS: usize = 10;
/// Artist mixes are built for the biggest artists only, beyond the requested
/// count by this much, so ranking them against genre mixes has some choice.
const ARTIST_MIX_SLACK: usize = 4;

/// One song as the mix engine sees it. Strings are interned so scoring a
/// hundred thousand songs compares integers.
struct MixTrack {
    id: i64,
    /// First artist named on the song, else none.
    primary: Option<u32>,
    album_artist: Option<u32>,
    /// The artist an album files under, as in `ArtistSummary`.
    group: u32,
    album: u32,
    genre: Option<u32>,
    year: Option<i32>,
    rating: u8,
    plays: u32,
    enabled: bool,
}

#[derive(Default)]
struct Interner {
    ids: HashMap<String, u32>,
    names: Vec<String>,
}

impl Interner {
    fn intern(&mut self, text: &str) -> u32 {
        if let Some(id) = self.ids.get(text) {
            return *id;
        }
        let id = self.names.len() as u32;
        self.names.push(text.to_owned());
        self.ids.insert(text.to_owned(), id);
        id
    }
}

/// Artist credits parsed once per distinct string, since a library repeats a
/// handful of thousand credits across a hundred thousand rows.
#[derive(Default)]
struct Credits {
    artists: Interner,
    parsed: HashMap<String, Vec<u32>>,
}

impl Credits {
    fn names(&mut self, credit: &str) -> &[u32] {
        if !self.parsed.contains_key(credit) {
            let ids = artist_names(credit)
                .iter()
                .map(|name| self.artists.intern(name))
                .collect();
            self.parsed.insert(credit.to_owned(), ids);
        }
        &self.parsed[credit]
    }
}

/// Borrowed straight from the row: interning only allocates for new strings.
fn text<'a>(row: &'a Row<'_>, index: usize) -> rusqlite::Result<&'a str> {
    Ok(row.get_ref(index)?.as_str()?)
}

fn optional_text<'a>(row: &'a Row<'_>, index: usize) -> rusqlite::Result<Option<&'a str>> {
    Ok(row.get_ref(index)?.as_str_or_null()?)
}

struct Catalog {
    /// Every music song, in id order.
    tracks: Vec<MixTrack>,
    by_id: HashMap<i64, usize>,
    genres: Interner,
    /// Each genre's display spelling, alphabetically first as in `genres()`.
    genre_names: Vec<String>,
    groups: Interner,
    /// How many songs credit two artists together, keyed by the lower id first.
    co_artists: HashMap<(u32, u32), u32>,
}

impl Catalog {
    fn load(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare(
            "SELECT id, artist, album_artist, artist_group, album_group, key_genre,
                    year, rating, play_count, enabled, genre
             FROM tracks WHERE kind = ?1 ORDER BY id",
        )?;
        let mut rows = stmt.query([MediaKind::Music])?;
        let mut catalog = Self {
            tracks: Vec::new(),
            by_id: HashMap::new(),
            genres: Interner::default(),
            genre_names: Vec::new(),
            groups: Interner::default(),
            co_artists: HashMap::new(),
        };
        let mut credits = Credits::default();
        let mut albums = Interner::default();
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let names = credits.names(text(row, 1)?).to_vec();
            let primary = names.first().copied();
            let album_artist = credits.names(text(row, 2)?).first().copied();
            if names.len() > 1 || album_artist.is_some_and(|a| Some(a) != primary) {
                let mut together = names;
                together.extend(album_artist);
                together.sort_unstable();
                together.dedup();
                for (i, first) in together.iter().enumerate() {
                    for second in &together[i + 1..] {
                        *catalog.co_artists.entry((*first, *second)).or_default() += 1;
                    }
                }
            }
            let genre = optional_text(row, 5)?.map(|key| catalog.genres.intern(key));
            if let (Some(genre), Some(spelling)) = (genre, optional_text(row, 10)?) {
                match catalog.genre_names.get_mut(genre as usize) {
                    Some(shown) if spelling < shown.as_str() => *shown = spelling.to_owned(),
                    Some(_) => {}
                    None => catalog.genre_names.push(spelling.to_owned()),
                }
            }
            let track = MixTrack {
                id,
                primary,
                album_artist,
                group: catalog.groups.intern(text(row, 3)?),
                album: albums.intern(text(row, 4)?),
                genre,
                year: row.get(6)?,
                rating: row.get(7)?,
                plays: row.get(8)?,
                enabled: row.get(9)?,
            };
            catalog.by_id.insert(id, catalog.tracks.len());
            catalog.tracks.push(track);
        }
        Ok(catalog)
    }

    fn artists_of(track: &MixTrack) -> impl Iterator<Item = u32> {
        let second = track.album_artist.filter(|a| Some(*a) != track.primary);
        track.primary.into_iter().chain(second)
    }
}

/// What the library knows about how songs are used together.
struct History {
    /// Track ids of each listening session, in play order.
    sessions: Vec<Vec<i64>>,
    /// Where each track occurs: (session, position in it).
    appearances: HashMap<i64, Vec<(usize, usize)>>,
    playlists: Vec<Vec<i64>>,
    memberships: HashMap<i64, Vec<usize>>,
}

impl History {
    fn load(conn: &Connection) -> Result<Self> {
        let mut history = Self {
            sessions: Vec::new(),
            appearances: HashMap::new(),
            playlists: Vec::new(),
            memberships: HashMap::new(),
        };

        let mut plays = conn
            .prepare("SELECT track_id, at FROM play_events WHERE kind = ?1 ORDER BY at, rowid")?;
        let mut previous_at: Option<i64> = None;
        for play in plays.query_map([PLAY_EVENT], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })? {
            let (track_id, at) = play?;
            if previous_at.is_none_or(|previous| at - previous >= SESSION_GAP_SECONDS) {
                history.sessions.push(Vec::new());
            }
            previous_at = Some(at);
            let session = history.sessions.len() - 1;
            let position = history.sessions[session].len();
            history.sessions[session].push(track_id);
            history
                .appearances
                .entry(track_id)
                .or_default()
                .push((session, position));
        }

        let mut items = conn.prepare(
            "SELECT playlist_id, track_id FROM playlist_items ORDER BY playlist_id, position",
        )?;
        let mut current: Option<i64> = None;
        for item in items.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))? {
            let (playlist_id, track_id) = item?;
            if current != Some(playlist_id) {
                current = Some(playlist_id);
                history.playlists.push(Vec::new());
            }
            let playlist = history.playlists.len() - 1;
            // A song listed twice is still one link.
            if !history.playlists[playlist].contains(&track_id) {
                history.playlists[playlist].push(track_id);
                history
                    .memberships
                    .entry(track_id)
                    .or_default()
                    .push(playlist);
            }
        }
        Ok(history)
    }
}

/// The seeds, boiled down to what candidates are compared against. Everything
/// here is built by iterating seeds and sources in a fixed order, so scores come
/// out bit-identical from run to run.
struct Profile {
    seeds: f64,
    /// How many seeds carry each genre.
    genres: HashMap<u32, u32>,
    /// How many seeds involve each artist.
    artists: HashMap<u32, u32>,
    mean_year: Option<f64>,
    /// Plays within a window of a seed, by track id.
    co_play: HashMap<i64, u32>,
    /// Weight of the playlists a track shares with a seed, by track id.
    playlists: HashMap<i64, f64>,
}

impl Profile {
    fn new(catalog: &Catalog, history: &History, seeds: &[usize]) -> Self {
        let mut profile = Self {
            seeds: seeds.len() as f64,
            genres: HashMap::new(),
            artists: HashMap::new(),
            mean_year: None,
            co_play: HashMap::new(),
            playlists: HashMap::new(),
        };
        let mut years = Vec::new();
        for &index in seeds {
            let seed = &catalog.tracks[index];
            if let Some(genre) = seed.genre {
                *profile.genres.entry(genre).or_default() += 1;
            }
            for artist in Catalog::artists_of(seed) {
                *profile.artists.entry(artist).or_default() += 1;
            }
            years.extend(seed.year);

            for &(session, position) in history.appearances.get(&seed.id).into_iter().flatten() {
                let plays = &history.sessions[session];
                let from = position.saturating_sub(CO_PLAY_WINDOW);
                let to = (position + CO_PLAY_WINDOW).min(plays.len() - 1);
                for (at, track_id) in plays.iter().enumerate().take(to + 1).skip(from) {
                    if at != position {
                        *profile.co_play.entry(*track_id).or_default() += 1;
                    }
                }
            }
            for &playlist in history.memberships.get(&seed.id).into_iter().flatten() {
                let members = &history.playlists[playlist];
                // A sprawling playlist says little about any two of its songs.
                let weight = 1.0 / (1.0 + members.len() as f64 / 25.0);
                for member in members {
                    *profile.playlists.entry(*member).or_default() += weight;
                }
            }
        }
        if !years.is_empty() {
            profile.mean_year = Some(f64::from(years.iter().sum::<i32>()) / years.len() as f64);
        }
        profile
    }

    /// The song's similarity to the seeds, or none if nothing links them.
    fn score(&self, catalog: &Catalog, track: &MixTrack) -> Option<f64> {
        let mut value = 0.0;
        let mut linked = false;

        if let Some(count) = track.genre.and_then(|genre| self.genres.get(&genre)) {
            value += WEIGHTS.genre * f64::from(*count) / self.seeds;
            linked = true;
        }
        let shared = Catalog::artists_of(track)
            .filter_map(|artist| self.artists.get(&artist))
            .max();
        if let Some(count) = shared {
            value += WEIGHTS.same_artist * f64::from(*count) / self.seeds;
            linked = true;
        } else {
            let joint = Catalog::artists_of(track)
                .flat_map(|artist| self.artists.keys().map(move |seed| (artist, *seed)))
                .filter_map(|(a, b)| catalog.co_artists.get(&(a.min(b), a.max(b))))
                .max();
            if let Some(count) = joint {
                value += WEIGHTS.co_artist * f64::from((*count).min(CO_ARTIST_CAP))
                    / f64::from(CO_ARTIST_CAP);
                linked = true;
            }
        }
        if let Some(count) = self.co_play.get(&track.id) {
            let count = f64::from(*count);
            value += WEIGHTS.co_play * count / (count + 2.0);
            linked = true;
        }
        if let Some(weight) = self.playlists.get(&track.id) {
            value += WEIGHTS.playlist * weight.min(1.0);
            linked = true;
        }
        if !linked {
            return None;
        }
        if let (Some(year), Some(mean)) = (track.year, self.mean_year) {
            let apart = (f64::from(year) - mean).abs();
            value += WEIGHTS.year * (1.0 - apart / YEAR_SPAN).max(0.0);
        }
        Some(value + WEIGHTS.rating * f64::from(track.rating) / 100.0)
    }
}

/// Candidates that link to the profile, best first, ties by id.
fn rank(
    catalog: &Catalog,
    profile: &Profile,
    pool: impl Iterator<Item = usize>,
) -> Vec<(f64, usize)> {
    let mut ranked: Vec<(f64, usize)> = pool
        .filter_map(|index| {
            let track = &catalog.tracks[index];
            profile.score(catalog, track).map(|score| (score, index))
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(catalog.tracks[a.1].id.cmp(&catalog.tracks[b.1].id))
    });
    ranked
}

/// Walks the ranking, taking the best song that keeps the mix varied: at most
/// two songs per artist in any ten in a row, and never one album twice running.
/// Stops short rather than break the rules when the pool lacks the variety.
fn pick(catalog: &Catalog, ranked: &[(f64, usize)], size: usize) -> Vec<(f64, usize)> {
    let mut chosen: Vec<(f64, usize)> = Vec::with_capacity(size);
    let mut taken = vec![false; ranked.len()];
    let mut first_free = 0;
    while chosen.len() < size {
        while taken.get(first_free) == Some(&true) {
            first_free += 1;
        }
        let recent = &chosen[chosen.len().saturating_sub(DIVERSITY_WINDOW - 1)..];
        let allowed = |index: usize| {
            let track = &catalog.tracks[index];
            let repeats_album = chosen
                .last()
                .is_some_and(|(_, last)| catalog.tracks[*last].album == track.album);
            let crowded = track.primary.is_some_and(|artist| {
                recent
                    .iter()
                    .filter(|(_, other)| catalog.tracks[*other].primary == Some(artist))
                    .count()
                    >= MAX_PER_ARTIST_IN_WINDOW
            });
            !repeats_album && !crowded
        };
        let Some(next) = (first_free..ranked.len()).find(|i| !taken[*i] && allowed(ranked[*i].1))
        else {
            break;
        };
        taken[next] = true;
        chosen.push(ranked[next]);
    }
    chosen
}

impl Library {
    /// "More like this": up to `size` songs ranked by similarity to the seeds,
    /// seeds themselves excluded. Unchecked songs, podcasts and audiobooks are
    /// never offered. A song must share a genre, an artist or collaborator, a
    /// listening session or a playlist with a seed to qualify, so the result
    /// can be shorter than `size`. Deterministic for a given library state.
    pub fn mix_for(&self, seeds: &[i64], size: usize) -> Result<Vec<i64>> {
        if size == 0 {
            return Ok(Vec::new());
        }
        let catalog = Catalog::load(&self.conn)?;
        let mut seed_indexes: Vec<usize> = Vec::new();
        for id in seeds {
            if let Some(index) = catalog.by_id.get(id)
                && !seed_indexes.contains(index)
            {
                seed_indexes.push(*index);
            }
        }
        if seed_indexes.is_empty() {
            return Ok(Vec::new());
        }

        let history = History::load(&self.conn)?;
        let profile = Profile::new(&catalog, &history, &seed_indexes);
        let pool = (0..catalog.tracks.len())
            .filter(|index| catalog.tracks[*index].enabled && !seed_indexes.contains(index));
        let ranked = rank(&catalog, &profile, pool);
        Ok(pick(&catalog, &ranked, size)
            .into_iter()
            .map(|(_, index)| catalog.tracks[index].id)
            .collect())
    }

    /// Up to `count` themed mixes of `size` songs: one per genre with enough
    /// songs, and one per big artist ("Miles Davis & More") built from their
    /// best songs and whatever sits near them. Biggest clusters first, the more
    /// cohesive first among equals. Clusters that cannot fill `size` within
    /// the variety rules are skipped, so fewer than `count` may come back.
    pub fn mixes(&self, count: usize, size: usize) -> Result<Vec<Mix>> {
        if count == 0 || size == 0 {
            return Ok(Vec::new());
        }
        let catalog = Catalog::load(&self.conn)?;
        let history = History::load(&self.conn)?;

        let mut by_genre: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        let mut by_group: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (index, track) in catalog.tracks.iter().enumerate().filter(|(_, t)| t.enabled) {
            if let Some(genre) = track.genre {
                by_genre.entry(genre).or_default().push(index);
            }
            by_group.entry(track.group).or_default().push(index);
        }

        let mut clusters: Vec<Cluster> = by_genre
            .into_iter()
            .filter(|(_, members)| members.len() >= size)
            .map(|(genre, members)| Cluster {
                key: format!("genre:{}", catalog.genres.names[genre as usize]),
                kind: ClusterKind::Genre(genre),
                members,
            })
            .collect();
        let mut artists: Vec<Cluster> = by_group
            .into_iter()
            .filter(|(group, members)| {
                members.len() >= ARTIST_MIX_MIN_TRACKS
                    && !matches!(
                        catalog.groups.names[*group as usize].as_str(),
                        "" | COMPILATIONS_GROUP
                    )
            })
            .map(|(group, members)| Cluster {
                key: format!("artist:{}", catalog.groups.names[group as usize]),
                kind: ClusterKind::Artist(group),
                members,
            })
            .collect();
        artists.sort_by(|a, b| {
            b.members
                .len()
                .cmp(&a.members.len())
                .then(a.key.cmp(&b.key))
        });
        artists.truncate(count + ARTIST_MIX_SLACK);
        clusters.extend(artists);

        // Biggest first: once enough distinct mixes are in hand, every cluster
        // still waiting is smaller, so it could not make the cut.
        clusters.sort_by(|a, b| {
            b.members
                .len()
                .cmp(&a.members.len())
                .then(a.key.cmp(&b.key))
        });
        let mut pending = clusters.into_iter().peekable();
        let mut built: Vec<Built> = Vec::new();
        let mut keep: Vec<usize>;
        loop {
            if let Some(group) = pending.peek().map(|cluster| cluster.members.len()) {
                while let Some(cluster) = pending.next_if(|c| c.members.len() == group) {
                    built.extend(build(&catalog, &history, cluster, size));
                }
            }
            built.sort_by(|a, b| {
                b.cluster
                    .members
                    .len()
                    .cmp(&a.cluster.members.len())
                    .then(b.quality.total_cmp(&a.quality))
                    .then(a.cluster.key.cmp(&b.cluster.key))
            });
            keep = distinct(&catalog, &built);
            if keep.len() >= count || pending.peek().is_none() {
                break;
            }
        }

        keep.truncate(count);
        let mut mixes = Vec::with_capacity(keep.len());
        for index in keep {
            let Built {
                cluster, chosen, ..
            } = &built[index];
            mixes.push(Mix {
                name: self.mix_name(&catalog, cluster)?,
                key: cluster.key.clone(),
                track_ids: chosen.iter().map(|(_, i)| catalog.tracks[*i].id).collect(),
                artwork_track_ids: artwork_for(&catalog, chosen),
            });
        }
        Ok(mixes)
    }

    /// Named as the rest of the library shows it: the first spelling of the genre
    /// or artist, alphabetically.
    fn mix_name(&self, catalog: &Catalog, cluster: &Cluster) -> Result<String> {
        Ok(match cluster.kind {
            ClusterKind::Genre(genre) => format!("{} Mix", catalog.genre_names[genre as usize]),
            ClusterKind::Artist(group) => {
                let artist = self
                    .conn
                    .query_row(
                        "SELECT MIN(CASE WHEN album_artist = '' THEN artist ELSE album_artist END)
                         FROM tracks WHERE artist_group = ?1",
                        [&catalog.groups.names[group as usize]],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten()
                    .unwrap_or_default();
                format!("{artist} & More")
            }
        })
    }
}

/// A cluster's finished mix, kept with what orders it among the others.
struct Built {
    quality: f64,
    cluster: Cluster,
    chosen: Vec<(f64, usize)>,
}

/// None when the cluster cannot fill the mix within the variety rules.
fn build(catalog: &Catalog, history: &History, cluster: Cluster, size: usize) -> Option<Built> {
    let mut best_first = cluster.members.clone();
    best_first.sort_by_key(|index| {
        let track = &catalog.tracks[*index];
        (
            std::cmp::Reverse(u32::from(track.rating) * 100 + track.plays.min(99)),
            track.id,
        )
    });
    best_first.truncate(CLUSTER_SEEDS);
    let profile = Profile::new(catalog, history, &best_first);
    let ranked = match cluster.kind {
        // Staying inside the genre is what makes it a genre mix.
        ClusterKind::Genre(_) => rank(catalog, &profile, cluster.members.iter().copied()),
        ClusterKind::Artist(_) => rank(
            catalog,
            &profile,
            (0..catalog.tracks.len()).filter(|i| catalog.tracks[*i].enabled),
        ),
    };
    let chosen = pick(catalog, &ranked, size);
    if chosen.len() < size {
        return None;
    }
    let quality = chosen.iter().map(|(score, _)| score).sum::<f64>() / size as f64;
    Some(Built {
        quality,
        cluster,
        chosen,
    })
}

/// Positions of the mixes that are not the same set of songs as an earlier one.
fn distinct(catalog: &Catalog, built: &[Built]) -> Vec<usize> {
    let mut seen: HashSet<Vec<i64>> = HashSet::new();
    (0..built.len())
        .filter(|index| {
            let mut ids: Vec<i64> = built[*index]
                .chosen
                .iter()
                .map(|(_, i)| catalog.tracks[*i].id)
                .collect();
            ids.sort_unstable();
            seen.insert(ids)
        })
        .collect()
}

struct Cluster {
    key: String,
    kind: ClusterKind,
    members: Vec<usize>,
}

enum ClusterKind {
    Genre(u32),
    Artist(u32),
}

/// A song from each of the first few distinct albums, in mix order.
fn artwork_for(catalog: &Catalog, chosen: &[(f64, usize)]) -> Vec<i64> {
    let mut albums = HashSet::new();
    chosen
        .iter()
        .map(|(_, index)| &catalog.tracks[*index])
        .filter(|track| albums.insert(track.album))
        .map(|track| track.id)
        .take(MIX_ARTWORK)
        .collect()
}
