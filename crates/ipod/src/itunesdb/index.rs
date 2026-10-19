//! The sort indexes (mhod 52) and jump tables (mhod 53) of the master playlist, from libgpod's
//! `mk_mhod` (`MHOD_ID_LIBPLAYLISTINDEX` and `MHOD_ID_LIBPLAYLISTJUMPTABLE`) and its comparison
//! functions. The iPod lists songs, artists, albums, genres and composers from these, so a stale
//! index shows up as wrong ordering or missing entries.

use std::cmp::Ordering;

use unicode_normalization::UnicodeNormalization;

use super::mhod::{self, u32_from};
use super::view::TrackView;

const SORT_TITLE: u32 = 0x03;
const SORT_ALBUM: u32 = 0x04;
const SORT_ARTIST: u32 = 0x05;
const SORT_GENRE: u32 = 0x07;
const SORT_COMPOSER: u32 = 0x12;

/// A string reduced to what the iPod sorts by: case and accents ignored. libgpod uses GLib's
/// locale-dependent collation keys; this keeps the same intent without depending on the locale.
fn fold(text: &str) -> String {
    text.nfd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The upper-cased first letter or digit class the jump bar groups by: `0` for digits and for
/// strings without any alphanumeric character.
fn jump_letter(folded: &str) -> u16 {
    let Some(first) = folded.chars().find(|c| c.is_alphanumeric()) else {
        return u16::from(b'0');
    };
    if !first.is_alphabetic() {
        return u16::from(b'0');
    }
    let upper = first.to_uppercase().next().unwrap_or(first);
    let mut units = [0u16; 2];
    upper.encode_utf16(&mut units)[0]
}

/// The artist as iTunes sorts it: `The Beatles` becomes `Beatles, The`.
fn artist_sort_text(view: &TrackView) -> String {
    if !view.sort_artist.is_empty() {
        return view.sort_artist.clone();
    }
    match view.artist.get(..4) {
        Some(prefix) if prefix.eq_ignore_ascii_case("the ") => {
            format!("{}, The\u{1}\u{1}\u{1}\u{1}\u{1}", &view.artist[4..])
        }
        _ => view.artist.clone(),
    }
}

fn prefer<'a>(sort: &'a str, plain: &'a str) -> &'a str {
    if sort.is_empty() { plain } else { sort }
}

struct Key {
    text: String,
    letter: u16,
}

impl Key {
    fn new(text: &str) -> Self {
        let text = fold(text);
        let letter = jump_letter(&text);
        Self { text, letter }
    }
}

struct Entry {
    title: Key,
    album: Key,
    artist: Key,
    genre: Key,
    composer: Key,
    track_number: u32,
    disc_number: u32,
}

impl Entry {
    fn new(view: &TrackView) -> Self {
        Self {
            title: Key::new(prefer(&view.sort_title, &view.title)),
            album: Key::new(prefer(&view.sort_album, &view.album)),
            artist: Key::new(&artist_sort_text(view)),
            genre: Key::new(&view.genre),
            composer: Key::new(prefer(&view.sort_composer, &view.composer)),
            track_number: view.track_number,
            disc_number: view.disc_number,
        }
    }

    fn by_position(&self, other: &Self) -> Ordering {
        self.disc_number
            .cmp(&other.disc_number)
            .then(self.track_number.cmp(&other.track_number))
            .then_with(|| self.title.text.cmp(&other.title.text))
    }
}

type Compare = fn(&Entry, &Entry) -> Ordering;
type Letter = fn(&Entry) -> u16;

fn by_title(a: &Entry, b: &Entry) -> Ordering {
    a.title.text.cmp(&b.title.text)
}

fn by_album(a: &Entry, b: &Entry) -> Ordering {
    a.album
        .text
        .cmp(&b.album.text)
        .then_with(|| a.by_position(b))
}

fn by_artist(a: &Entry, b: &Entry) -> Ordering {
    a.artist
        .text
        .cmp(&b.artist.text)
        .then_with(|| by_album(a, b))
}

fn by_genre(a: &Entry, b: &Entry) -> Ordering {
    a.genre
        .text
        .cmp(&b.genre.text)
        .then_with(|| by_artist(a, b))
}

fn by_composer(a: &Entry, b: &Entry) -> Ordering {
    a.composer
        .text
        .cmp(&b.composer.text)
        .then_with(|| by_album(a, b))
}

/// The ten mhods, in libgpod's order: an index then a jump table for title, artist, album, genre
/// and composer. Indexes refer to positions in `members`, the master playlist's order.
pub(super) fn mhods(members: &[&TrackView]) -> Vec<Vec<u8>> {
    let entries: Vec<Entry> = members.iter().map(|view| Entry::new(view)).collect();
    let sorts: [(u32, Compare, Letter); 5] = [
        (SORT_TITLE, by_title, |e| e.title.letter),
        (SORT_ARTIST, by_artist, |e| e.artist.letter),
        (SORT_ALBUM, by_album, |e| e.album.letter),
        (SORT_GENRE, by_genre, |e| e.genre.letter),
        (SORT_COMPOSER, by_composer, |e| e.composer.letter),
    ];
    sorts
        .into_iter()
        .flat_map(|(sort, compare, letter)| {
            let mut order: Vec<usize> = (0..entries.len()).collect();
            order.sort_by(|&a, &b| compare(&entries[a], &entries[b]));
            let letters: Vec<u16> = order.iter().map(|&index| letter(&entries[index])).collect();
            [index_mhod(sort, &order), jump_table_mhod(sort, &letters)]
        })
        .collect()
}

fn index_mhod(sort: u32, order: &[usize]) -> Vec<u8> {
    let mut mhod = mhod::header(mhod::INDEX, 48 + 4 * order.len());
    mhod.extend_from_slice(&sort.to_le_bytes());
    mhod.extend_from_slice(&u32_from(order.len()).to_le_bytes());
    mhod.extend_from_slice(&[0; 40]);
    for &index in order {
        mhod.extend_from_slice(&u32_from(index).to_le_bytes());
    }
    mhod
}

/// One entry per run of equal letters in the sorted order: the letter, where the run starts and
/// how long it is.
fn jump_table_mhod(sort: u32, letters: &[u16]) -> Vec<u8> {
    let mut runs: Vec<(u16, u32, u32)> = Vec::new();
    for (position, &letter) in letters.iter().enumerate() {
        match runs.last_mut() {
            Some((last, _, count)) if *last == letter => *count += 1,
            _ => runs.push((letter, u32_from(position), 1)),
        }
    }
    let mut mhod = mhod::header(mhod::JUMP_TABLE, 16 + 12 * runs.len());
    mhod.extend_from_slice(&sort.to_le_bytes());
    mhod.extend_from_slice(&u32_from(runs.len()).to_le_bytes());
    mhod.extend_from_slice(&[0; 8]);
    for (letter, start, count) in runs {
        mhod.extend_from_slice(&letter.to_le_bytes());
        mhod.extend_from_slice(&[0, 0]);
        mhod.extend_from_slice(&start.to_le_bytes());
        mhod.extend_from_slice(&count.to_le_bytes());
    }
    mhod
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(title: &str, artist: &str, track_number: u32) -> TrackView {
        TrackView {
            title: title.into(),
            artist: artist.into(),
            album: "A".into(),
            album_artist: String::new(),
            genre: String::new(),
            composer: String::new(),
            tv_show: String::new(),
            sort_title: String::new(),
            sort_artist: String::new(),
            sort_album: String::new(),
            sort_album_artist: String::new(),
            sort_composer: String::new(),
            track_number,
            disc_number: 1,
        }
    }

    #[test]
    fn folding_ignores_case_and_accents() {
        assert_eq!(fold("Beyoncé"), fold("BEYONCE"));
        assert!(fold("Zebra") > fold("école"));
    }

    #[test]
    fn jump_letters_group_digits_and_symbols_under_zero() {
        assert_eq!(jump_letter(&fold("42 things")), u16::from(b'0'));
        assert_eq!(jump_letter(&fold("...!")), u16::from(b'0'));
        assert_eq!(jump_letter(&fold("¡hola")), u16::from(b'H'));
        assert_eq!(jump_letter(&fold("école")), u16::from(b'E'));
    }

    #[test]
    fn the_prefix_moves_to_the_end_when_sorting_artists() {
        let zombies = Entry::new(&view("x", "The Zombies", 1));
        let yes = Entry::new(&view("x", "Yes", 1));

        // Sorted as "Zombies, The", which comes after "Yes"; as typed, "the zombies" would not.
        assert_eq!(by_artist(&zombies, &yes), Ordering::Greater);
    }

    #[test]
    fn index_lists_member_positions_in_sorted_order_with_matching_jump_tables() {
        let tracks = [
            view("Charlie", "x", 1),
            view("alpha", "x", 2),
            view("Bravo", "x", 3),
        ];
        let members: Vec<&TrackView> = tracks.iter().collect();

        let mhods = mhods(&members);

        assert_eq!(mhods.len(), 10);
        let index = &mhods[0];
        assert_eq!(u32_at(index, 12), Some(mhod::INDEX));
        assert_eq!(u32_at(index, 24), Some(SORT_TITLE));
        assert_eq!(u32_at(index, 28), Some(3));
        let order: Vec<u32> = (0..3).map(|i| u32_at(index, 72 + 4 * i).unwrap()).collect();
        assert_eq!(order, [1, 2, 0]);
        let jump = &mhods[1];
        assert_eq!(u32_at(jump, 12), Some(mhod::JUMP_TABLE));
        assert_eq!(u32_at(jump, 28), Some(3));
    }

    use crate::bytes::u32_at;
}
