//! Finding the file for each song, strictest rule first. Every file is given to at most one
//! song, and a rule only ever acts on a song and a file that have no other candidate for each
//! other: where it would be a guess, the song is left unmatched.

use std::collections::HashMap;

use super::index::{File, Index};
use super::names::{folder_name, loose, track_stems};
use super::{Confidence, LocateProgress, LocateStage, Located, LocatedFile};
use crate::item::{MusicLibraryItem, Ownership};

/// A file's size says its length only roughly: tags and artwork add to it and VBR blurs it.
/// Two seconds, or this share of the song if that is more, is allowed either way.
const DURATION_SLACK_MS: u64 = 2_000;
const DURATION_SLACK_PERCENT: u64 = 3;
/// The bitrates a file of unknown encoding might have, in bytes per millisecond, for lossy
/// formats and for uncompressed ones (CD and 48 kHz stereo).
const LOSSY_RATES: [f64; 6] = [16.0, 20.0, 24.0, 28.0, 32.0, 40.0];
const UNCOMPRESSED_RATES: [f64; 2] = [176.4, 192.0];
/// Shorter names than this say too little for one to start another.
const MIN_PREFIX: usize = 5;
const MIN_CONFIDENT_PREFIX: usize = 12;
const MIN_SUFFIX: usize = 4;
const MIN_GLOBAL_TITLE: usize = 3;

/// What is known about a song that has to be found.
struct Wanted {
    item: usize,
    stems: Vec<String>,
    title: String,
    /// The folders it may be in, best first, and whether each is the one Music would have made.
    albums: Vec<(usize, bool)>,
}

pub(crate) fn run(
    index: &Index,
    items: &[MusicLibraryItem],
    progress: &mut dyn FnMut(LocateProgress),
) -> Located {
    let mut matcher = Matcher {
        index,
        items,
        claimed: vec![false; index.files.len()],
        found: vec![None; items.len()],
        owner: vec![None; index.files.len()],
    };
    let wanted: Vec<Wanted> = items
        .iter()
        .enumerate()
        // A song streamed from Apple Music has no file to find.
        .filter(|(_, item)| !item.file_present && item.ownership != Ownership::Subscription)
        .map(|(n, item)| matcher.describe(n, item))
        .collect();
    let total = wanted.len();
    let mut report = |done: usize| {
        progress(LocateProgress {
            stage: LocateStage::Matching,
            done,
            total: Some(total),
        });
    };

    matcher.names_music_would_give(&wanted, &mut report);
    matcher.in_the_right_folder(&wanted);
    matcher.title_elsewhere(&wanted);
    report(total);

    let mut located = Located::default();
    for want in &wanted {
        let item = &items[want.item];
        match matcher.found[want.item] {
            Some((file, confidence)) => {
                located.matched.insert(
                    item.persistent_id.clone(),
                    LocatedFile {
                        path: index.files[file].path.clone(),
                        confidence,
                        dataless: index.files[file].dataless,
                    },
                );
            }
            None => located.unmatched.push(item.persistent_id.clone()),
        }
    }
    located
}

struct Matcher<'a> {
    index: &'a Index,
    items: &'a [MusicLibraryItem],
    claimed: Vec<bool>,
    /// For each song, its file and how sure the match is.
    found: Vec<Option<(usize, Confidence)>>,
    /// For each file, the song it was given to.
    owner: Vec<Option<usize>>,
}

impl Matcher<'_> {
    fn describe(&self, n: usize, item: &MusicLibraryItem) -> Wanted {
        let filed_under = if item.album_artist.trim().is_empty() {
            &item.artist
        } else {
            &item.album_artist
        };
        let mut artists = vec![folder_name(filed_under, "Unknown Artist")];
        // Other places a song of a compilation, or by an artist renamed since, may be.
        if !item.artist.trim().is_empty() {
            artists.push(folder_name(&item.artist, "Unknown Artist"));
        }
        artists.push("Compilations".to_owned());
        let album = folder_name(&item.album, "Unknown Album");
        Wanted {
            item: n,
            albums: self.index.albums_for(&artists, &album),
            stems: track_stems(&item.title, item.track_number, item.disc_number),
            title: loose(&item.title),
        }
    }

    fn claim(&mut self, want: &Wanted, file: usize, confidence: Confidence) {
        self.claimed[file] = true;
        self.owner[file] = Some(want.item);
        self.found[want.item] = Some((file, confidence));
    }

    /// Rule one: the file is where Music would have put it, under the name Music would have given
    /// it. Songs added earlier take the plain name and later copies the numbered ones, as Music
    /// did; so every song is offered the plain names before any is offered a numbered one.
    fn names_music_would_give(&mut self, wanted: &[Wanted], report: &mut dyn FnMut(usize)) {
        let mut order: Vec<&Wanted> = wanted.iter().collect();
        order.sort_by_key(|want| {
            (
                self.items[want.item].date_added.unwrap_or(i64::MAX),
                want.item,
            )
        });
        for duplicates in [false, true] {
            for (done, want) in order.iter().enumerate() {
                if !duplicates && done % 1_000 == 0 {
                    report(done);
                }
                if self.found[want.item].is_some() {
                    continue;
                }
                let hit = want.albums.iter().find_map(|&(album, made_by_music)| {
                    want.stems
                        .iter()
                        .find_map(|stem| {
                            self.take_stem(album, stem, duplicates, &self.items[want.item])
                        })
                        .map(|file| (file, made_by_music))
                });
                if let Some((file, made_by_music)) = hit {
                    let confidence = if made_by_music {
                        Confidence::Exact
                    } else {
                        Confidence::Folder
                    };
                    self.claim(want, file, confidence);
                }
            }
        }
    }

    /// A free file called `stem`; or, if `duplicates`, `stem 1`, `stem 2`... which is how Music
    /// numbers a file whose name was taken.
    fn take_stem(
        &self,
        album: usize,
        stem: &str,
        duplicates: bool,
        item: &MusicLibraryItem,
    ) -> Option<usize> {
        let by_stem = &self.index.albums[album].by_stem;
        let names: Vec<String> = if duplicates {
            (1..=9).map(|n| format!("{stem} {n}")).collect()
        } else {
            vec![stem.to_owned()]
        };
        names
            .iter()
            .filter_map(|name| by_stem.get(&super::names::fold(name)))
            .find_map(|ids| self.pick(ids.iter().copied(), item))
    }

    /// Of files that differ only in format, the free one that suits the song's kind.
    fn pick(&self, ids: impl Iterator<Item = usize>, item: &MusicLibraryItem) -> Option<usize> {
        let free: Vec<usize> = ids.filter(|&id| !self.claimed[id]).collect();
        free.iter()
            .copied()
            .find(|&id| suits(&self.index.files[id].extension, &item.kind))
            .or_else(|| free.first().copied())
    }

    /// Rules two to six, within the folder the song should be in, for songs whose file has
    /// another name than Music would give it: renamed by another tool, retitled since, cut short.
    fn in_the_right_folder(&mut self, wanted: &[Wanted]) {
        type Rule = fn(&Wanted, &File, &Context) -> bool;
        const RULES: [Rule; 5] = [
            |w, f, c| !w.title.is_empty() && (f.title == w.title || c.number_run_into(w, f)),
            |w, f, c| c.starts_the_other(w, f),
            |w, f, _| {
                w.title.len() >= MIN_SUFFIX && f.whole.ends_with(&w.title) && f.title != f.whole
            },
            |_, f, c| c.same_number(f) && c.plausible_length(f),
            |_, f, c| c.calibrated && c.plausible_length(f),
        ];
        for rule in RULES {
            self.apply(wanted, rule);
        }
        self.last_pair(wanted);
    }

    fn apply(&mut self, wanted: &[Wanted], rule: fn(&Wanted, &File, &Context) -> bool) {
        for (album, group) in self.pending_by_album(wanted) {
            let mut proposals: Vec<(&Wanted, usize)> = Vec::new();
            for want in group {
                let context = Context::of(self, album, &self.items[want.item]);
                let fits: Vec<usize> = self.index.albums[album]
                    .files
                    .iter()
                    .copied()
                    .filter(|&id| !self.claimed[id] && rule(want, &self.index.files[id], &context))
                    .collect();
                if let Some(file) = self.single(&fits, want) {
                    proposals.push((want, file));
                }
            }
            // A file wanted by two songs goes to the one whose track number it carries, if exactly
            // one does; otherwise it is a coin toss and neither gets it.
            let mut wanted_by: HashMap<usize, Vec<&Wanted>> = HashMap::new();
            for (want, file) in proposals {
                wanted_by.entry(file).or_default().push(want);
            }
            for (file, rivals) in wanted_by {
                let winner = match rivals.as_slice() {
                    [only] => Some(*only),
                    many => {
                        let numbered: Vec<&&Wanted> = many
                            .iter()
                            .filter(|want| self.has_number_of(want, file))
                            .collect();
                        match numbered.as_slice() {
                            [one] => Some(**one),
                            _ => None,
                        }
                    }
                };
                if let Some(want) = winner {
                    self.claim(want, file, Confidence::Folder);
                }
            }
        }
    }

    /// Whether the file's name starts with the number of the song's track.
    fn has_number_of(&self, want: &Wanted, file: usize) -> bool {
        let track = self.items[want.item].track_number;
        track.is_some() && self.index.files[file].number.map(|(_, number)| number) == track
    }

    /// The one file out of several that is the song's, if something tells them apart: the
    /// number in its name, then the format.
    fn single(&self, fits: &[usize], want: &Wanted) -> Option<usize> {
        let item = &self.items[want.item];
        match fits {
            [] => None,
            [only] => Some(*only),
            many => {
                let by_number: Vec<usize> = many
                    .iter()
                    .copied()
                    .filter(|&id| {
                        item.track_number.is_some()
                            && self.index.files[id].number.map(|(_, track)| track)
                                == item.track_number
                    })
                    .collect();
                let narrowed = if by_number.is_empty() {
                    many.to_vec()
                } else {
                    by_number
                };
                let by_format: Vec<usize> = narrowed
                    .iter()
                    .copied()
                    .filter(|&id| suits(&self.index.files[id].extension, &item.kind))
                    .collect();
                match (by_format.as_slice(), narrowed.as_slice()) {
                    ([only], _) | ([], [only]) => Some(*only),
                    _ => None,
                }
            }
        }
    }

    /// Songs not yet found, by the folder they should be in.
    fn pending_by_album<'w>(&self, wanted: &'w [Wanted]) -> Vec<(usize, Vec<&'w Wanted>)> {
        let mut groups: HashMap<usize, Vec<&Wanted>> = HashMap::new();
        for want in wanted {
            if self.found[want.item].is_none()
                && let Some(&(album, _)) = want.albums.first()
            {
                groups.entry(album).or_default().push(want);
            }
        }
        let mut groups: Vec<_> = groups.into_iter().collect();
        groups.sort_by_key(|(album, _)| *album);
        groups
    }

    /// Rule six: one song and one file left in a folder, and the file is the right size for it.
    fn last_pair(&mut self, wanted: &[Wanted]) {
        for (album, group) in self.pending_by_album(wanted) {
            let free: Vec<usize> = self.index.albums[album]
                .files
                .iter()
                .copied()
                .filter(|&id| !self.claimed[id])
                .collect();
            if let ([want], [file]) = (group.as_slice(), free.as_slice()) {
                let context = Context::of(self, album, &self.items[want.item]);
                if context.plausible_length(&self.index.files[*file]) {
                    self.claim(want, *file, Confidence::Folder);
                }
            }
        }
    }

    /// Rule seven: no folder agrees, but only one file anywhere has this title, and only one
    /// song is looking for it.
    fn title_elsewhere(&mut self, wanted: &[Wanted]) {
        let mut by_title: HashMap<&str, Vec<&Wanted>> = HashMap::new();
        for want in wanted {
            if self.found[want.item].is_none() && want.title.len() >= MIN_GLOBAL_TITLE {
                by_title.entry(&want.title).or_default().push(want);
            }
        }
        for (title, wants) in by_title {
            if let ([want], [file]) = (wants.as_slice(), self.index.with_title(title))
                && !self.claimed[*file]
            {
                self.claim(want, *file, Confidence::Fuzzy);
            }
        }
    }
}

/// What the matched songs of a folder say about the rest of it.
struct Context {
    /// Bytes of file per millisecond of song, as this folder's matched files show.
    rate: Option<f64>,
    calibrated: bool,
    duration_ms: Option<u64>,
    track: Option<u32>,
    disc: Option<u32>,
}

impl Context {
    /// The folder's calibration, with the facts about the song being looked for.
    fn of(matcher: &Matcher, album: usize, item: &MusicLibraryItem) -> Self {
        let mut rates: Vec<f64> = matcher.index.albums[album]
            .files
            .iter()
            .filter_map(|&id| {
                let item = &matcher.items[matcher.owner[id]?];
                let duration = item.duration_ms.filter(|&ms| ms >= 5_000)?;
                Some(matcher.index.files[id].size as f64 / duration as f64)
            })
            .collect();
        rates.sort_by(f64::total_cmp);
        let rate = rates.get(rates.len() / 2).copied();
        Self {
            rate,
            calibrated: rate.is_some(),
            duration_ms: item.duration_ms,
            track: item.track_number,
            disc: item.disc_number,
        }
    }

    /// The file's name starts with the song's title or the title with the file's, and the
    /// numbers do not disagree, unless the shared part is long enough to be no accident.
    fn starts_the_other(&self, want: &Wanted, file: &File) -> bool {
        let (title, name) = (&want.title, &file.title);
        let shorter = title.len().min(name.len());
        shorter >= MIN_PREFIX
            && (title.starts_with(name.as_str()) || name.starts_with(title.as_str()))
            && (shorter >= MIN_CONFIDENT_PREFIX || self.same_number(file))
    }

    /// The name is the title with the track number jammed against it: `02Charms Alarm`.
    fn number_run_into(&self, want: &Wanted, file: &File) -> bool {
        self.track
            .is_some_and(|track| file.whole == format!("{track:02}{}", want.title))
    }

    fn same_number(&self, file: &File) -> bool {
        let Some(track) = self.track else {
            return false;
        };
        match file.number {
            Some((disc, number)) => {
                number == track && (disc.is_none() || self.disc.is_none() || disc == self.disc)
            }
            None => false,
        }
    }

    /// The file is about the size a song of this length would be. Without a rate from the
    /// folder's own files, any usual bitrate will do.
    fn plausible_length(&self, file: &File) -> bool {
        let Some(duration) = self.duration_ms else {
            return false;
        };
        let slack = DURATION_SLACK_MS.max(duration * DURATION_SLACK_PERCENT / 100) as f64;
        let rates: &[f64] = match (self.rate, file.extension.as_str()) {
            (Some(rate), _) => &[rate],
            (None, "wav" | "aif" | "aiff") => &UNCOMPRESSED_RATES,
            (None, _) => &LOSSY_RATES,
        };
        rates
            .iter()
            .any(|rate| (file.size as f64 - rate * duration as f64).abs() <= rate * slack)
    }
}

/// Whether a file extension is one a song of this iTunes kind has.
fn suits(extension: &str, kind: &str) -> bool {
    let kind = kind.to_lowercase();
    let wanted: &[&str] = if kind.contains("mpeg audio") {
        &["mp3"]
    } else if kind.contains("protected") {
        &["m4p"]
    } else if kind.contains("aac") || kind.contains("apple lossless") {
        &["m4a", "m4p", "aac"]
    } else if kind.contains("aiff") {
        &["aif", "aiff"]
    } else if kind.contains("wav") {
        &["wav"]
    } else if kind.contains("video") {
        &["m4v", "mp4"]
    } else {
        &[]
    };
    wanted.contains(&extension)
}
