use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::cloud::split_track_prefix;
use crate::edit::{EditReport, TrackEdits};
use crate::error::Result;
use crate::library::Library;
use crate::model::{Track, TrackQuery};
use crate::normalize::{fold, is_release_note, is_year};

/// Tags that come from where a file was found rather than from the song. One goes only when
/// it is the whole of a bracketed group, in any case, so Live, Remix, Acoustic, feat. and
/// Remastered, which name a real version, stay. A bitrate such as "320 kbps" goes too.
const JUNK_TAGS: &[&str] = &[
    "official video",
    "official music video",
    "official audio",
    "official lyric video",
    "official visualizer",
    "music video",
    "lyric video",
    "lyrics",
    "audio",
    "visualizer",
    "hq",
    "hd",
    "high quality",
    "free download",
];
const SITE_ENDINGS: [&str; 3] = [".com", ".net", ".org"];
const DASHES: [&str; 3] = [" - ", " – ", " — "];
/// What a name that opens a note about the recording, not the song, starts with: "Acoustic",
/// "Demo". `normalize` has the notes that are whole phrases ("2009 Remaster", "Mono Version"),
/// and "Live" needs the word after it to tell it from the song ("Live Forever").
const VERSION_WORDS: &[&str] = &["version", "mix", "edit", "demo", "acoustic", "instrumental"];
/// "Live at Wembley", but not "Live and Let Die".
const LIVE_FOLLOWERS: &[&str] = &["at", "in", "from", "on"];
/// Left in lower case by Title Case, unless first or last.
const SMALL_WORDS: &[&str] = &[
    "a", "an", "the", "and", "but", "or", "nor", "of", "in", "on", "at", "to", "for", "by", "vs",
];
/// Lower case in any case style, unless they open the text: the "o'" and "'n'" of "Sweet Child
/// o' Mine" and "Rock 'n' Roll".
const ALWAYS_LOWER: &[&str] = &["o", "n"];
/// Lower case likewise, but only as the abbreviation "feat." or "ft.", not in "Little Feat".
const LOWER_ABBREVIATIONS: &[&str] = &["feat", "ft"];

/// The text field Clean Up Song Info works on. Only the name can lose a track number or an artist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CleanUpField {
    #[default]
    Title,
    Artist,
    Album,
    AlbumArtist,
    Composer,
    Genre,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextSide {
    Start,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveCharacters {
    pub count: usize,
    pub from: TextSide,
}

/// An empty `find` replaces nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReplaceText {
    pub find: String,
    pub with: String,
    pub match_case: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Capitalization {
    Title,
    Sentence,
    Lower,
    Upper,
}

/// What to do to one field of each song. The operations that are on run in the order
/// they are declared here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CleanUpRules {
    pub field: CleanUpField,
    /// Underscores and `%20` become spaces, and runs of spaces become one.
    pub tidy: bool,
    pub remove_junk: bool,
    /// Name only. A number fills the song's track (or disc) number when it has none.
    pub remove_track_numbers: bool,
    /// Name only. "Artist - Name" loses the artist when it is the song's own, or is
    /// taken as the artist of a song that has none.
    pub remove_artist: bool,
    pub remove_characters: Option<RemoveCharacters>,
    pub replace: Option<ReplaceText>,
    pub capitalize: Option<Capitalization>,
}

/// One song the rules would change, for the before and after list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanUpChange {
    pub id: i64,
    pub before: String,
    pub after: String,
    /// Taken from the name, for a song with no artist.
    pub artist: Option<String>,
    /// Taken from the name, only for a song with no track (or disc) number.
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    /// The song is a placeholder for a file in the cloud, so the change can only be kept in the
    /// library.
    pub in_cloud: bool,
}

impl Library {
    /// The songs among `ids` the rules would change, in the order given; nothing is written.
    /// Stations are skipped, and so is a song whose field would end up empty.
    pub fn clean_up_preview(
        &self,
        ids: &[i64],
        rules: &CleanUpRules,
    ) -> Result<Vec<CleanUpChange>> {
        let query = TrackQuery {
            kind: None,
            ids: Some(ids.to_vec()),
            ..TrackQuery::default()
        };
        let mut tracks: HashMap<i64, Track> = self
            .tracks(&query)?
            .into_iter()
            .map(|track| (track.id, track))
            .collect();
        Ok(ids
            .iter()
            .filter_map(|id| tracks.remove(id))
            .filter_map(|track| rules.change_for(&track))
            .collect())
    }

    /// Writes what `clean_up_preview` lists, a song at a time because each has its own values,
    /// and stops before the next song once `cancel` is set. `progress` is told how many of the
    /// songs to change are done.
    pub fn clean_up(
        &mut self,
        ids: &[i64],
        rules: &CleanUpRules,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<EditReport> {
        let changes = self.clean_up_preview(ids, rules)?;
        let total = changes.len() as u64;
        let mut report = EditReport::default();
        for (index, change) in changes.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            report.merge(self.edit_tracks(&[change.id], &change.edits(rules.field))?);
            progress(index as u64 + 1, total);
        }
        Ok(report)
    }
}

impl CleanUpChange {
    /// Whether it adds details the song lacked, besides rewriting a field.
    pub fn fills(&self) -> bool {
        self.artist.is_some() || self.track_number.is_some() || self.disc_number.is_some()
    }

    fn edits(&self, field: CleanUpField) -> TrackEdits {
        let mut edits = TrackEdits {
            artist: self.artist.clone(),
            track_number: self.track_number.map(Some),
            disc_number: self.disc_number.map(Some),
            ..TrackEdits::default()
        };
        let after = Some(self.after.clone());
        match field {
            CleanUpField::Title => edits.title = after,
            CleanUpField::Artist => edits.artist = after,
            CleanUpField::Album => edits.album = after,
            CleanUpField::AlbumArtist => edits.album_artist = after,
            CleanUpField::Composer => edits.composer = after,
            CleanUpField::Genre => edits.genre = after,
        }
        edits
    }
}

impl CleanUpRules {
    fn change_for(&self, track: &Track) -> Option<CleanUpChange> {
        let before = match self.field {
            CleanUpField::Title => &track.title,
            CleanUpField::Artist => &track.artist,
            CleanUpField::Album => &track.album,
            CleanUpField::AlbumArtist => &track.album_artist,
            CleanUpField::Composer => &track.composer,
            CleanUpField::Genre => &track.genre,
        };
        let mut change = CleanUpChange {
            id: track.id,
            before: before.clone(),
            after: before.clone(),
            artist: None,
            track_number: None,
            disc_number: None,
            in_cloud: track.in_cloud,
        };
        if self.tidy {
            change.after = tidy(&change.after);
        }
        if self.remove_junk {
            change.after = remove_junk(&change.after);
        }
        if self.field == CleanUpField::Title {
            if self.remove_track_numbers
                && let Some((disc, number, name)) = split_track_prefix(&change.after)
            {
                // A zero ("00 Intro") is a way of writing no number.
                let fill = |number: u32, has: Option<u32>| {
                    Some(number).filter(|n| *n > 0 && has.is_none())
                };
                change.disc_number = disc.and_then(|disc| fill(disc, track.disc_number));
                change.track_number = fill(number, track.track_number);
                change.after = name;
            }
            if self.remove_artist
                && let Some((artist, name)) = split_artist(&change.after, track)
            {
                change.artist = artist;
                change.after = name;
            }
        }
        if let Some(removal) = &self.remove_characters {
            change.after = removal.apply(&change.after);
        }
        if let Some(replacement) = &self.replace {
            change.after = replacement.apply(&change.after);
        }
        if let Some(style) = self.capitalize {
            change.after = capitalize(&change.after, style);
        }
        // `edit_tracks` trims what it stores, so the preview shows the trimmed text too. Stray
        // space around a name nothing else touched is not a change worth listing.
        change.after = change.after.trim().to_owned();
        (!change.after.is_empty() && (change.after != change.before.trim() || change.fills()))
            .then_some(change)
    }
}

fn tidy(text: &str) -> String {
    text.replace('_', " ")
        .replace("%20", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Takes out every junk tag and web address, with the separator left dangling by each.
fn remove_junk(text: &str) -> String {
    let Some(span) = find_junk(text) else {
        return text.to_owned();
    };
    let mut text = cut(text, span);
    while let Some(span) = find_junk(&text) {
        text = cut(&text, span);
    }
    text.trim_matches(|c: char| c.is_whitespace() || "-–—:|,".contains(c))
        .to_owned()
}

fn find_junk(text: &str) -> Option<Range<usize>> {
    bracketed_junk(text).or_else(|| web_address(text))
}

fn bracketed_junk(text: &str) -> Option<Range<usize>> {
    text.char_indices().find_map(|(start, open)| {
        let close = match open {
            '(' => ')',
            '[' => ']',
            '{' => '}',
            _ => return None,
        };
        let end = group_end(text, start, open, close)?;
        let tag = &text[start + 1..end];
        (is_junk_tag(tag) || is_website(tag.trim())).then_some(start..end + 1)
    })
}

/// Where the group opened at `start` closes, counting the groups nested in it.
fn group_end(text: &str, start: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0;
    for (index, c) in text[start..].char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(start + index);
            }
        }
    }
    None
}

fn is_junk_tag(tag: &str) -> bool {
    let tag = tag
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let is_bitrate = tag.strip_suffix("kbps").is_some_and(|digits| {
        let digits = digits.trim_end();
        (2..=3).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
    });
    JUNK_TAGS.contains(&tag.as_str()) || is_bitrate
}

fn is_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '.'
}

fn is_website(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    SITE_ENDINGS.iter().any(|ending| {
        text.strip_suffix(ending).is_some_and(|host| {
            host.starts_with(|c: char| c.is_ascii_alphanumeric()) && host.chars().all(is_host_char)
        })
    })
}

/// A `www.` address anywhere, or a site name that is a whole dashed piece of the text
/// ("Song - example.com"); a bare "example.com" inside a name is more likely the song's.
fn web_address(text: &str) -> Option<Range<usize>> {
    let mut start = None;
    for (index, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match (is_host_char(c), start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                if is_website(&text[from..index])
                    && let Some(span) = standalone_site(text, from..index)
                {
                    return Some(span);
                }
                start = None;
            }
            _ => {}
        }
    }
    None
}

/// The site at `site` with the dash that joins it to the rest, if it should go.
fn standalone_site(text: &str, site: Range<usize>) -> Option<Range<usize>> {
    if text[site.clone()].to_ascii_lowercase().starts_with("www.") {
        return Some(site);
    }
    let (before, after) = (&text[..site.start], &text[site.end..]);
    let dash_before = DASHES.iter().find(|dash| before.ends_with(**dash));
    let dash_after = DASHES.iter().find(|dash| after.starts_with(**dash));
    match (dash_before, dash_after) {
        (Some(dash), _) if after.is_empty() || dash_after.is_some() => {
            Some(site.start - dash.len()..site.end)
        }
        (None, Some(dash)) if before.is_empty() => Some(site.start..site.end + dash.len()),
        _ => None,
    }
}

/// Removes `span`, leaving one space where text remains on both sides, unless the text on
/// one side is a bracket or punctuation that belongs against what it follows.
fn cut(text: &str, span: Range<usize>) -> String {
    let (head, tail) = (text[..span.start].trim_end(), text[span.end..].trim_start());
    let joined = head.is_empty()
        || tail.is_empty()
        || head.ends_with(['(', '[', '{'])
        || tail.starts_with([')', ']', '}', ',', ';', ':']);
    format!("{head}{}{tail}", if joined { "" } else { " " })
}

/// "Artist - Name" as the new name and the artist to fill in: none when the song has the
/// artist (or album artist) already, the prefix when it has no artist. Anything else, like
/// "Yesterday - Remastered 2009" under another artist, is a name that merely has a dash, and
/// so is "Iron Maiden - Live", whose rest is a note about the recording, not a song.
fn split_artist(name: &str, track: &Track) -> Option<(Option<String>, String)> {
    let (at, separator) = DASHES
        .iter()
        .filter_map(|separator| name.find(separator).map(|at| (at, separator)))
        .min_by_key(|(at, _)| *at)?;
    let (prefix, rest) = (name[..at].trim(), name[at + separator.len()..].trim());
    if prefix.is_empty() || rest.is_empty() || is_version_note(rest) {
        return None;
    }
    let is_prefix = |known: &str| !known.is_empty() && fold(known) == fold(prefix);
    if is_prefix(&track.artist) || is_prefix(&track.album_artist) {
        Some((None, rest.to_owned()))
    } else if track.artist.is_empty() {
        Some((Some(prefix.to_owned()), rest.to_owned()))
    } else {
        None
    }
}

/// A year on its own is a song's name ("Prince - 1999"), but "1999 Mix" is a note.
fn is_version_note(text: &str) -> bool {
    let text = fold(text);
    if is_release_note(&text) {
        return true;
    }
    let mut words = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty() && !is_year(word));
    match words.next() {
        Some("live") => words
            .next()
            .is_none_or(|next| LIVE_FOLLOWERS.contains(&next)),
        Some(first) => VERSION_WORDS.contains(&first),
        None => false,
    }
}

impl RemoveCharacters {
    fn apply(&self, text: &str) -> String {
        match self.from {
            TextSide::Start => text.chars().skip(self.count).collect(),
            TextSide::End => {
                let kept = text.chars().count().saturating_sub(self.count);
                text.chars().take(kept).collect()
            }
        }
    }
}

impl ReplaceText {
    fn apply(&self, text: &str) -> String {
        if self.find.is_empty() {
            text.to_owned()
        } else if self.match_case {
            text.replace(&self.find, &self.with)
        } else {
            let mut replaced = String::with_capacity(text.len());
            let mut rest = text;
            while let Some(c) = rest.chars().next() {
                match self.matched_length(rest) {
                    Some(length) => {
                        replaced.push_str(&self.with);
                        rest = &rest[length..];
                    }
                    None => {
                        replaced.push(c);
                        rest = &rest[c.len_utf8()..];
                    }
                }
            }
            replaced
        }
    }

    /// How many bytes of `text` the search text matches at its start, ignoring case.
    fn matched_length(&self, text: &str) -> Option<usize> {
        let mut chars = text.char_indices();
        let mut length = 0;
        for wanted in self.find.chars() {
            let (index, c) = chars.next()?;
            if !c.to_lowercase().eq(wanted.to_lowercase()) {
                return None;
            }
            length = index + c.len_utf8();
        }
        Some(length)
    }
}

fn capitalize(text: &str, style: Capitalization) -> String {
    match style {
        Capitalization::Lower => text.to_lowercase(),
        Capitalization::Upper => text.to_uppercase(),
        Capitalization::Title | Capitalization::Sentence => {
            let words = word_spans(text);
            let places: Vec<Place> = (0..words.len())
                .map(|n| place_of(text, &words, n))
                .collect();
            // The letters of "R.E.M." are cased already, and do not make a phrase.
            let cased = words
                .iter()
                .zip(&places)
                .filter(|(word, place)| {
                    !place.initialism
                        && text[(*word).clone()]
                            .chars()
                            .any(|c| c.is_uppercase() || c.is_lowercase())
                })
                .count();
            // A name in capitals is a name ("ABBA", "AC/DC"); only a phrase is shouting.
            let shouting = cased >= 3 && !text.chars().any(char::is_lowercase);
            let mut recased = String::with_capacity(text.len());
            let mut copied = 0;
            for (span, place) in words.iter().zip(&places) {
                recased.push_str(&text[copied..span.start]);
                recased.push_str(&recase(&text[span.clone()], style, shouting, place));
                copied = span.end;
            }
            recased.push_str(&text[copied..]);
            recased
        }
    }
}

fn place_of(text: &str, words: &[Range<usize>], n: usize) -> Place {
    let word = &words[n];
    let before = &text[words[..n].last().map_or(0, |previous| previous.end)..word.start];
    let after = &text[word.end..words.get(n + 1).map_or(text.len(), |next| next.start)];
    Place {
        start: n == 0,
        first: n == 0 || opens_part(before),
        last: n + 1 == words.len() || closes_part(after),
        lone: before.chars().next_back().is_none_or(char::is_whitespace)
            && after.chars().next().is_none_or(char::is_whitespace),
        initialism: text[word.clone()].chars().count() == 1
            && (before.ends_with('.') || after.starts_with('.')),
        abbreviated: after.starts_with('.'),
    }
}

/// A spaced hyphen divides a title like a dash does; one inside a word ("Mid-Tempo") does not.
fn is_spaced_hyphen(gap: &str) -> bool {
    gap.contains('-') && gap.contains(char::is_whitespace)
}

/// The text before a word starts a new part of the title: a bracket, a colon or a dash.
fn opens_part(gap: &str) -> bool {
    gap.contains(['(', '[', '{', ':', '–', '—']) || is_spaced_hyphen(gap)
}

/// The text after a word ends a part of the title.
fn closes_part(gap: &str) -> bool {
    gap.contains([')', ']', '}', ':', '–', '—']) || is_spaced_hyphen(gap)
}

/// Runs of letters and digits; an apostrophe between them stays inside ("don't").
fn word_spans(text: &str) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut spans = Vec::new();
    let mut start = None;
    for (n, &(index, c)) in chars.iter().enumerate() {
        let inside = c.is_alphanumeric()
            || (matches!(c, '\'' | '’')
                && start.is_some()
                && chars
                    .get(n + 1)
                    .is_some_and(|(_, next)| next.is_alphanumeric()));
        match (inside, start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                spans.push(from..index);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        spans.push(from..text.len());
    }
    spans
}

/// Where a word stands in the text.
struct Place {
    /// The first word of the whole text.
    start: bool,
    /// The first word of the text or of a part of it, after a bracket, a colon or a dash.
    first: bool,
    last: bool,
    /// Set off from the words around it by space, or by the edge of the text.
    lone: bool,
    /// A letter of "A.C." or "R.E.M.".
    initialism: bool,
    /// Followed by a full stop, as in "feat." and "ft.".
    abbreviated: bool,
}

enum WordCase {
    /// No letters with a case: digits, or a script without one.
    Uncased,
    Lower,
    Upper,
    /// "Hello": a capital first, and no other capitals.
    Capitalized,
    /// "iPhone", "McCartney", "2Pac": someone chose this spelling, so it is left alone.
    Mixed,
}

impl WordCase {
    fn of(word: &str) -> Self {
        let (upper, lower) = (
            word.chars().any(char::is_uppercase),
            word.chars().any(char::is_lowercase),
        );
        match (upper, lower) {
            (false, false) => Self::Uncased,
            (false, true) => Self::Lower,
            (true, false) => Self::Upper,
            (true, true) => {
                let mut chars = word.chars();
                if chars.next().is_some_and(char::is_uppercase) && !chars.any(char::is_uppercase) {
                    Self::Capitalized
                } else {
                    Self::Mixed
                }
            }
        }
    }
}

/// Title Case lowers the small words that are not first or last in the text or a part of it,
/// and Sentence case every word but the first. A word in capitals is taken for an acronym
/// ("USA", "DJ") unless the whole text is shouting.
fn recase(word: &str, style: Capitalization, shouting: bool, place: &Place) -> String {
    // A lone "A" is a word like any other, but the one in "A$AP" and "A&M" is a letter of a name.
    let case = if word == "A" && place.lone {
        WordCase::Capitalized
    } else {
        WordCase::of(word)
    };
    let keep = match case {
        WordCase::Uncased | WordCase::Mixed => true,
        WordCase::Upper => !shouting || place.initialism,
        WordCase::Lower | WordCase::Capitalized => false,
    };
    if keep {
        return word.to_owned();
    }
    let lowered = word.to_lowercase();
    // "I", "I'm" and "I'll" are written in capitals in any case.
    let pronoun = lowered == "i" || lowered.starts_with("i'") || lowered.starts_with("i’");
    let lower_word = ALWAYS_LOWER.contains(&lowered.as_str())
        || (place.abbreviated && LOWER_ABBREVIATIONS.contains(&lowered.as_str()));
    let capital = if lower_word {
        place.start
    } else if style == Capitalization::Title {
        place.first || place.last || !SMALL_WORDS.contains(&lowered.as_str())
    } else {
        place.first || pronoun
    };
    if !capital {
        return lowered;
    }
    let mut chars = lowered.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> CleanUpRules {
        CleanUpRules::default()
    }

    fn song(title: &str, artist: &str) -> Track {
        Track {
            id: 1,
            path: "/music/1.mp3".into(),
            title: title.into(),
            artist: artist.into(),
            album_artist: String::new(),
            album: String::new(),
            genre: String::new(),
            composer: String::new(),
            year: None,
            track_number: None,
            track_count: None,
            disc_number: None,
            disc_count: None,
            duration_ms: 1,
            bitrate_kbps: None,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            codec: "MP3".into(),
            file_size: 1,
            date_added: 0,
            date_modified: 0,
            play_count: 0,
            last_played: None,
            skip_count: 0,
            rating: 0,
            kind: crate::model::MediaKind::Music,
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

    /// What the rules make of a song's name, or `None` when they leave it out.
    fn after(rules: &CleanUpRules, title: &str) -> Option<String> {
        rules
            .change_for(&song(title, ""))
            .map(|change| change.after)
    }

    #[test]
    fn tidy_turns_underscores_and_escapes_into_single_spaces() {
        let rules = CleanUpRules {
            tidy: true,
            ..rules()
        };

        assert_eq!(
            after(&rules, "  My_Song%20Name   (live) ").as_deref(),
            Some("My Song Name (live)")
        );
        assert_eq!(after(&rules, "Already Tidy"), None);
    }

    #[test]
    fn junk_tags_go_in_any_bracket_and_any_case() {
        let rules = CleanUpRules {
            remove_junk: true,
            ..rules()
        };
        let cases = [
            ("Halo (Official Video)", "Halo"),
            ("Halo [OFFICIAL MUSIC VIDEO]", "Halo"),
            ("Halo {official  audio}", "Halo"),
            ("Halo (Official Lyric Video)", "Halo"),
            ("Halo (Lyric Video)", "Halo"),
            ("Halo (Lyrics)", "Halo"),
            ("Halo (Audio)", "Halo"),
            ("Halo [Visualizer]", "Halo"),
            ("Halo (HQ) [HD]", "Halo"),
            ("Halo [320 kbps]", "Halo"),
            ("Halo (320kbps)", "Halo"),
            ("(Official Video) Halo", "Halo"),
            ("Halo (Official Video) Part 2", "Halo Part 2"),
            ("Halo - (Official Video)", "Halo"),
        ];
        for (before, expected) in cases {
            assert_eq!(after(&rules, before).as_deref(), Some(expected), "{before}");
        }
    }

    #[test]
    fn tags_that_name_a_real_version_are_kept() {
        let rules = CleanUpRules {
            remove_junk: true,
            ..rules()
        };

        for kept in [
            "Halo (Live)",
            "Halo (Live at the BBC)",
            "Halo (Remix)",
            "Halo [Acoustic]",
            "Halo (feat. Jay-Z)",
            "Halo (Remastered 2009)",
            "Halo (Audio Slave Mix)",
            "Audio",
            "HD Radio",
            "Halo (1999 kbps)",
        ] {
            assert_eq!(after(&rules, kept), None, "{kept}");
        }
        assert_eq!(
            after(&rules, "Halo (Live) (HD)").as_deref(),
            Some("Halo (Live)")
        );
    }

    #[test]
    fn junk_inside_other_brackets_goes_without_leaving_stray_spaces() {
        let rules = CleanUpRules {
            remove_junk: true,
            ..rules()
        };
        let cases = [
            ("Halo [Live (HD)]", "Halo [Live]"),
            ("Halo (Live (HD))", "Halo (Live)"),
            ("Halo (Live [HQ] Remix)", "Halo (Live Remix)"),
            ("Halo (HD), Part 2", "Halo, Part 2"),
            ("Halo (HD): Part 2", "Halo: Part 2"),
            ("Halo (www.site.com remix)", "Halo (remix)"),
            ("Halo ([HD] Remix)", "Halo (Remix)"),
            ("Halo (Official Video (HD))", "Halo"),
        ];
        for (before, expected) in cases {
            assert_eq!(after(&rules, before).as_deref(), Some(expected), "{before}");
        }
    }

    #[test]
    fn web_addresses_go_when_they_are_clearly_not_part_of_the_name() {
        let rules = CleanUpRules {
            remove_junk: true,
            ..rules()
        };
        let cases = [
            ("Halo www.example.com", "Halo"),
            ("WWW.Example.NET - Halo", "Halo"),
            ("Halo (www.example.org)", "Halo"),
            ("Halo [example.com]", "Halo"),
            ("Halo - example.com", "Halo"),
            ("example.net - Halo", "Halo"),
            ("Halo - example.org - Remix", "Halo - Remix"),
            ("Halo – example.com", "Halo"),
        ];
        for (before, expected) in cases {
            assert_eq!(after(&rules, before).as_deref(), Some(expected), "{before}");
        }
        for kept in [
            "Amazon.com",
            "Halo example.com again",
            "Halo - example.com Remix",
            "Halo (example.io)",
        ] {
            assert_eq!(after(&rules, kept), None, "{kept}");
        }
    }

    #[test]
    fn track_numbers_come_off_the_start_and_fill_empty_numbers() {
        let rules = CleanUpRules {
            remove_track_numbers: true,
            ..rules()
        };
        let change = |title: &str| rules.change_for(&song(title, "")).unwrap();

        let plain = change("01 - Song");
        assert_eq!(
            (plain.after.as_str(), plain.track_number, plain.disc_number),
            ("Song", Some(1), None)
        );
        let disc = change("1-03 Song");
        assert_eq!(
            (disc.after.as_str(), disc.track_number, disc.disc_number),
            ("Song", Some(3), Some(1))
        );
        assert_eq!(change("02. Song").after, "Song");
        assert_eq!(change("03 – Song").after, "Song");
        let intro = change("00 - Intro");
        assert_eq!((intro.after.as_str(), intro.track_number), ("Intro", None));
        assert_eq!(after(&rules, "7 Rings"), None);
        assert_eq!(after(&rules, "10 Years"), None);
        assert_eq!(after(&rules, "2Pac Intro"), None);
    }

    #[test]
    fn a_number_the_song_already_has_is_not_overwritten() {
        let rules = CleanUpRules {
            remove_track_numbers: true,
            ..rules()
        };
        let mut numbered = song("2-05 Song", "");
        numbered.track_number = Some(9);

        let change = rules.change_for(&numbered).unwrap();

        assert_eq!(change.after, "Song");
        assert_eq!(change.track_number, None);
        assert_eq!(change.disc_number, Some(2), "the disc was empty");
    }

    #[test]
    fn track_numbers_are_only_removed_from_names() {
        let rules = CleanUpRules {
            field: CleanUpField::Album,
            remove_track_numbers: true,
            ..rules()
        };
        let mut track = song("x", "");
        track.album = "01 - Album".into();

        assert_eq!(rules.change_for(&track), None);
    }

    #[test]
    fn the_artist_comes_off_the_name_when_it_is_the_songs_own() {
        let rules = CleanUpRules {
            remove_artist: true,
            ..rules()
        };
        let mut track = song("beyonce – Halo", "Beyoncé");

        let change = rules.change_for(&track).unwrap();
        assert_eq!((change.after.as_str(), change.artist), ("Halo", None));

        track.title = "Queen - Bohemian Rhapsody".into();
        track.artist = "Somebody".into();
        track.album_artist = "QUEEN".into();
        assert_eq!(rules.change_for(&track).unwrap().after, "Bohemian Rhapsody");
    }

    #[test]
    fn a_song_without_an_artist_takes_it_from_the_name() {
        let rules = CleanUpRules {
            remove_artist: true,
            ..rules()
        };

        let change = rules
            .change_for(&song("Queen — Bohemian Rhapsody", ""))
            .unwrap();

        assert_eq!(change.after, "Bohemian Rhapsody");
        assert_eq!(change.artist.as_deref(), Some("Queen"));
    }

    #[test]
    fn a_dash_that_does_not_follow_the_artist_is_left_alone() {
        let rules = CleanUpRules {
            remove_artist: true,
            ..rules()
        };

        assert_eq!(
            rules.change_for(&song("Yesterday - Remastered 2009", "The Beatles")),
            None
        );
        assert_eq!(rules.change_for(&song("Hello - ", "Adele")), None);
        assert_eq!(rules.change_for(&song("Jay-Z", "")), None);
    }

    #[test]
    fn a_colon_is_part_of_the_name_not_a_separator() {
        let rules = CleanUpRules {
            remove_artist: true,
            ..rules()
        };

        for name in [
            "Star Wars: Main Title",
            "Interlude: Rain",
            "Queen: Bohemian Rhapsody",
        ] {
            assert_eq!(rules.change_for(&song(name, "")), None, "{name}");
            assert_eq!(rules.change_for(&song(name, "Queen")), None, "{name}");
        }
    }

    #[test]
    fn a_note_about_the_recording_is_not_a_song_name() {
        let rules = CleanUpRules {
            remove_artist: true,
            ..rules()
        };
        let cases = [
            ("Black Sabbath - 2009 Remaster", "Black Sabbath"),
            ("Black Sabbath - Remastered", "Black Sabbath"),
            ("Black Sabbath - Remastered Edition", "Black Sabbath"),
            ("Yesterday - Remastered 2009", ""),
            ("Iron Maiden - Live", "Iron Maiden"),
            ("Iron Maiden - Live at Donington", "Iron Maiden"),
            ("Song - Live at Wembley", ""),
            ("Song - Radio Edit", ""),
            ("Song - Mono Version", ""),
            ("Song - Acoustic", ""),
            ("Song - 1999 Mix", ""),
            ("Song - Demo", ""),
            ("Song - Instrumental", ""),
        ];
        for (name, artist) in cases {
            assert_eq!(rules.change_for(&song(name, artist)), None, "{name}");
        }
        let change = rules
            .change_for(&song("Iron Maiden - Hallowed", "Iron Maiden"))
            .unwrap();
        assert_eq!(change.after, "Hallowed");
    }

    #[test]
    fn a_title_that_only_starts_like_a_note_is_still_a_title() {
        let rules = CleanUpRules {
            remove_artist: true,
            ..rules()
        };
        let cases = [
            ("Prince - 1999", "Prince", "1999"),
            (
                "The Smashing Pumpkins - 1979",
                "The Smashing Pumpkins",
                "1979",
            ),
            ("Pretenders - 2000 Miles", "Pretenders", "2000 Miles"),
            ("Oasis - Live Forever", "Oasis", "Live Forever"),
            ("Wings - Live and Let Die", "Wings", "Live and Let Die"),
            ("Edward Maya - Stereo Love", "Edward Maya", "Stereo Love"),
            ("Mono - Ashes in the Snow", "Mono", "Ashes in the Snow"),
        ];
        for (name, artist, expected) in cases {
            let change = rules.change_for(&song(name, artist)).unwrap();
            assert_eq!(change.after, expected, "{name}");
        }
        for note in [
            "Song - Live 1985",
            "Song - Live from Paris",
            "Song - Live on Tour",
            "Song - 1999 Mix",
        ] {
            assert_eq!(rules.change_for(&song(note, "")), None, "{note}");
        }
    }

    #[test]
    fn numbers_then_artist_are_both_taken_off() {
        let rules = CleanUpRules {
            remove_track_numbers: true,
            remove_artist: true,
            ..rules()
        };

        let change = rules
            .change_for(&song("04 - Queen - Bohemian Rhapsody", "Queen"))
            .unwrap();

        assert_eq!(
            (change.after.as_str(), change.track_number),
            ("Bohemian Rhapsody", Some(4))
        );
    }

    #[test]
    fn characters_are_removed_from_either_end() {
        let from_start = CleanUpRules {
            remove_characters: Some(RemoveCharacters {
                count: 3,
                from: TextSide::Start,
            }),
            ..rules()
        };
        let from_end = CleanUpRules {
            remove_characters: Some(RemoveCharacters {
                count: 4,
                from: TextSide::End,
            }),
            ..rules()
        };

        assert_eq!(after(&from_start, "01 Halo").as_deref(), Some("Halo"));
        assert_eq!(after(&from_end, "Halo 2009").as_deref(), Some("Halo"));
        assert_eq!(
            after(&from_start, "Émile").as_deref(),
            Some("le"),
            "counted in characters, not bytes"
        );
    }

    #[test]
    fn removing_more_than_there_is_leaves_the_song_out() {
        let rules = CleanUpRules {
            remove_characters: Some(RemoveCharacters {
                count: 50,
                from: TextSide::End,
            }),
            ..rules()
        };

        assert_eq!(after(&rules, "Halo"), None);
    }

    #[test]
    fn replace_changes_every_match_and_can_ignore_case() {
        let replace = |find: &str, with: &str, match_case| CleanUpRules {
            replace: Some(ReplaceText {
                find: find.into(),
                with: with.into(),
                match_case,
            }),
            ..rules()
        };

        assert_eq!(
            after(&replace("a", "4", true), "Banana A").as_deref(),
            Some("B4n4n4 A")
        );
        assert_eq!(
            after(&replace("a", "4", false), "Banana A").as_deref(),
            Some("B4n4n4 4")
        );
        assert_eq!(
            after(&replace(" - ", " ", true), "A - B - C").as_deref(),
            Some("A B C")
        );
        assert_eq!(
            after(&replace("É", "e", false), "Élan é").as_deref(),
            Some("elan e")
        );
        assert_eq!(
            after(&replace("(demo)", "", false), "Halo (DEMO)").as_deref(),
            Some("Halo")
        );
        assert_eq!(
            after(&replace("", "x", true), "Halo"),
            None,
            "an empty search is off"
        );
    }

    #[test]
    fn title_case_lowers_small_words_but_not_the_first_or_last() {
        let rules = CleanUpRules {
            capitalize: Some(Capitalization::Title),
            ..rules()
        };
        let cases = [
            ("the lord of the rings", "The Lord of the Rings"),
            ("what a wonderful world", "What a Wonderful World"),
            ("a day in the life", "A Day in the Life"),
            ("what is this for", "What Is This For"),
            ("don't stop believin'", "Don't Stop Believin'"),
            ("mid-tempo rock/pop", "Mid-Tempo Rock/Pop"),
            ("Ode To The Sea", "Ode to the Sea"),
            ("i'm here and i know", "I'm Here and I Know"),
            ("(live at the roxy)", "(Live at the Roxy)"),
        ];
        for (before, expected) in cases {
            assert_eq!(after(&rules, before).as_deref(), Some(expected), "{before}");
        }
    }

    #[test]
    fn title_case_leaves_words_someone_spelled_on_purpose() {
        let rules = CleanUpRules {
            capitalize: Some(Capitalization::Title),
            ..rules()
        };

        assert_eq!(
            after(&rules, "my iPhone and paul McCartney").as_deref(),
            Some("My iPhone and Paul McCartney")
        );
        assert_eq!(
            after(&rules, "songs by AC/DC and a DJ").as_deref(),
            Some("Songs by AC/DC and a DJ")
        );
        assert_eq!(
            after(&rules, "Born In The USA").as_deref(),
            Some("Born in the USA")
        );
        assert_eq!(after(&rules, "iPhone"), None);
        assert_eq!(after(&rules, "1st of 3"), None);
    }

    #[test]
    fn a_word_whose_first_character_is_not_a_capital_is_left_alone() {
        for style in [Capitalization::Title, Capitalization::Sentence] {
            let rules = CleanUpRules {
                capitalize: Some(style),
                ..rules()
            };

            assert_eq!(after(&rules, "2Pac and 東京Tokyo"), None, "{style:?}");
        }
    }

    #[test]
    fn a_part_in_brackets_or_after_a_colon_or_dash_has_its_own_first_and_last_word() {
        let rules = CleanUpRules {
            capitalize: Some(Capitalization::Title),
            ..rules()
        };
        for kept in [
            "Pride (In the Name of Love)",
            "The Lord of the Rings: The Two Towers",
            "Halo (The Remix)",
            "Halo [The Remix]",
            "Halo - The Remix",
            "Halo (Live at the Roxy)",
        ] {
            assert_eq!(after(&rules, kept), None, "{kept}");
        }
        let cases = [
            ("pride (in the name of love)", "Pride (In the Name of Love)"),
            (
                "the lord of the rings: the two towers",
                "The Lord of the Rings: The Two Towers",
            ),
            ("halo (the remix)", "Halo (The Remix)"),
            ("halo - the remix", "Halo - The Remix"),
            ("rock-and-roll", "Rock-and-Roll"),
        ];
        for (before, expected) in cases {
            assert_eq!(after(&rules, before).as_deref(), Some(expected), "{before}");
        }
    }

    #[test]
    fn feat_and_the_contractions_of_of_and_and_stay_lower_case() {
        let rules = CleanUpRules {
            capitalize: Some(Capitalization::Title),
            ..rules()
        };
        let cases = [
            ("halo (feat. jay-z)", "Halo (feat. Jay-Z)"),
            ("Halo (Feat. Jay-Z)", "Halo (feat. Jay-Z)"),
            ("halo ft. beyonce", "Halo ft. Beyonce"),
            ("sweet child o' mine", "Sweet Child o' Mine"),
            ("rock 'n' roll", "Rock 'n' Roll"),
            ("feat. nobody", "Feat. Nobody"),
        ];
        for (before, expected) in cases {
            assert_eq!(after(&rules, before).as_deref(), Some(expected), "{before}");
        }
        for kept in ["Halo (feat. Jay-Z)", "Sweet Child o' Mine", "Rock 'n' Roll"] {
            assert_eq!(after(&rules, kept), None, "{kept}");
        }
    }

    #[test]
    fn feat_is_lower_case_only_as_an_abbreviation() {
        let rules = CleanUpRules {
            capitalize: Some(Capitalization::Title),
            ..rules()
        };

        assert_eq!(after(&rules, "Little Feat"), None);
        assert_eq!(
            after(&rules, "little feat live").as_deref(),
            Some("Little Feat Live")
        );
        assert_eq!(
            after(&rules, "Halo (Feat. Jay-Z)").as_deref(),
            Some("Halo (feat. Jay-Z)")
        );
    }

    #[test]
    fn dotted_initialisms_are_already_cased() {
        for style in [Capitalization::Title, Capitalization::Sentence] {
            let rules = CleanUpRules {
                capitalize: Some(style),
                ..rules()
            };

            for name in ["R.E.M.", "S.O.S.", "D.A.N.C.E."] {
                assert_eq!(after(&rules, name), None, "{name} in {style:?}");
            }
        }
        let title = CleanUpRules {
            capitalize: Some(Capitalization::Title),
            ..rules()
        };
        // Three words in capitals besides the letters of R.E.M., so it is shouting.
        assert_eq!(
            after(&title, "THE BEST OF R.E.M.").as_deref(),
            Some("The Best of R.E.M.")
        );
    }

    #[test]
    fn a_lone_a_is_an_ordinary_word_but_not_a_letter_of_an_initialism() {
        let title = CleanUpRules {
            capitalize: Some(Capitalization::Title),
            ..rules()
        };
        let sentence = CleanUpRules {
            capitalize: Some(Capitalization::Sentence),
            ..rules()
        };

        assert_eq!(
            after(&title, "What A Wonderful World").as_deref(),
            Some("What a Wonderful World")
        );
        assert_eq!(
            after(&sentence, "Take A Chance On Me").as_deref(),
            Some("Take a chance on me")
        );
        assert_eq!(
            after(&title, "A Day In The Life").as_deref(),
            Some("A Day in the Life")
        );
        assert_eq!(after(&title, "A.C. Newman"), None);
        for kept in ["A$AP Rocky", "Halo (feat. A$AP Rocky)", "A&M Records"] {
            assert_eq!(after(&title, kept), None, "{kept}");
        }
        assert_eq!(
            after(&title, "halo (feat. A$AP Rocky)").as_deref(),
            Some("Halo (feat. A$AP Rocky)")
        );
        assert_eq!(
            after(&sentence, "A&M Records").as_deref(),
            Some("A&M records")
        );
        assert_eq!(
            after(&sentence, "Take A.C. Home").as_deref(),
            Some("Take A.C. home")
        );
    }

    #[test]
    fn a_title_in_capitals_is_recased_throughout() {
        let rules = CleanUpRules {
            capitalize: Some(Capitalization::Title),
            ..rules()
        };

        assert_eq!(
            after(&rules, "THE LORD OF THE RINGS").as_deref(),
            Some("The Lord of the Rings")
        );
        assert_eq!(
            after(&rules, "BORN IN THE USA").as_deref(),
            Some("Born in the Usa")
        );
        assert_eq!(after(&rules, "U2").as_deref(), None);
        assert_eq!(after(&rules, "I WANT YOU").as_deref(), Some("I Want You"));
    }

    #[test]
    fn a_short_name_in_capitals_is_a_name_not_shouting() {
        for style in [Capitalization::Title, Capitalization::Sentence] {
            let rules = CleanUpRules {
                capitalize: Some(style),
                ..rules()
            };

            for name in [
                "AC/DC",
                "ABBA",
                "INXS",
                "UB40",
                "EDM",
                "DAMN.",
                "BLACK SABBATH",
            ] {
                assert_eq!(after(&rules, name), None, "{name} in {style:?}");
            }
        }
    }

    #[test]
    fn sentence_case_capitalizes_only_the_first_word() {
        let rules = CleanUpRules {
            capitalize: Some(Capitalization::Sentence),
            ..rules()
        };

        assert_eq!(
            after(&rules, "The Lord Of The Rings").as_deref(),
            Some("The lord of the rings")
        );
        assert_eq!(
            after(&rules, "THE LORD OF THE RINGS").as_deref(),
            Some("The lord of the rings")
        );
        assert_eq!(
            after(&rules, "halo (live) with iPhone").as_deref(),
            Some("Halo (Live) with iPhone")
        );
        assert_eq!(
            after(&rules, "Hello I'm Here").as_deref(),
            Some("Hello I'm here")
        );
        assert_eq!(
            after(&rules, "USA For Africa").as_deref(),
            Some("USA for africa")
        );
    }

    #[test]
    fn lower_and_upper_case_change_every_letter() {
        let lower = CleanUpRules {
            capitalize: Some(Capitalization::Lower),
            ..rules()
        };
        let upper = CleanUpRules {
            capitalize: Some(Capitalization::Upper),
            ..rules()
        };

        assert_eq!(after(&lower, "Halo iPhone").as_deref(), Some("halo iphone"));
        assert_eq!(after(&upper, "Halo iPhone").as_deref(), Some("HALO IPHONE"));
    }

    #[test]
    fn operations_run_in_a_fixed_order() {
        let rules = CleanUpRules {
            tidy: true,
            remove_junk: true,
            remove_track_numbers: true,
            remove_characters: Some(RemoveCharacters {
                count: 2,
                from: TextSide::End,
            }),
            replace: Some(ReplaceText {
                find: "halo".into(),
                with: "the day of the week".into(),
                match_case: true,
            }),
            capitalize: Some(Capitalization::Title),
            ..rules()
        };

        // Tidy makes "01 halo (HD) x" of the name, so the number is found and the junk goes before
        // the last two characters do; the replacement is then capitalized, not the other way round.
        let change = rules.change_for(&song("01_halo_(HD)_x", "")).unwrap();

        assert_eq!(
            (change.after.as_str(), change.track_number),
            ("The Day of the Week", Some(1))
        );
    }

    #[test]
    fn a_field_that_would_end_up_empty_is_left_out() {
        let rules = CleanUpRules {
            remove_junk: true,
            ..rules()
        };

        assert_eq!(after(&rules, "(Official Video)"), None);
        assert_eq!(after(&rules, ""), None);
    }

    #[test]
    fn another_field_is_cleaned_on_its_own() {
        let rules = CleanUpRules {
            field: CleanUpField::Artist,
            capitalize: Some(Capitalization::Title),
            ..rules()
        };
        let change = rules.change_for(&song("title", "the cranberries")).unwrap();

        assert_eq!(
            (change.before.as_str(), change.after.as_str()),
            ("the cranberries", "The Cranberries")
        );
        assert_eq!(
            change.edits(CleanUpField::Artist).artist.as_deref(),
            Some("The Cranberries")
        );
        assert_eq!(change.edits(CleanUpField::Artist).title, None);
    }

    #[test]
    fn rules_are_read_from_the_camel_case_the_interface_sends() {
        let rules: CleanUpRules = serde_json::from_str(
            r#"{"field":"albumArtist","tidy":true,"removeCharacters":{"count":2,"from":"end"},
                "replace":{"find":"a","with":"b","matchCase":true},"capitalize":"sentence"}"#,
        )
        .unwrap();

        assert_eq!(rules.field, CleanUpField::AlbumArtist);
        assert!(rules.tidy && !rules.remove_junk);
        assert_eq!(
            rules.remove_characters,
            Some(RemoveCharacters {
                count: 2,
                from: TextSide::End
            })
        );
        assert_eq!(
            rules.replace,
            Some(ReplaceText {
                find: "a".into(),
                with: "b".into(),
                match_case: true
            })
        );
        assert_eq!(rules.capitalize, Some(Capitalization::Sentence));
        assert_eq!(
            serde_json::from_str::<CleanUpRules>("{}").unwrap(),
            CleanUpRules::default()
        );
    }
}
