use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const ARTICLES: [&str; 3] = ["the ", "a ", "an "];

/// Decorations that describe a release rather than the recording, so a
/// remaster or single edit is still the same song. Live, remix, acoustic,
/// demo, instrumental and other edits are different recordings and are not here.
const RELEASE_NOTES: &[&str] = &[
    "remaster",
    "remastered",
    "remastered version",
    "digitally remastered",
    "mono",
    "mono version",
    "stereo",
    "stereo version",
    "single",
    "single version",
    "album version",
    "explicit",
    "explicit version",
    "clean",
    "clean version",
    "radio edit",
    "bonus track",
    "deluxe",
    "deluxe edition",
    "deluxe version",
];
const FEATURE_MARKERS: &[&str] = &[" feat. ", " feat ", " ft. ", " ft ", " featuring "];
const FEATURE_PREFIXES: &[&str] = &["feat.", "feat ", "ft.", "ft ", "featuring "];
const CO_ARTIST_MARKERS: &[&str] = &[" & ", ", ", " x ", " with "];
/// Placeholders that say nothing about who made the song.
const ANONYMOUS_ARTISTS: &[&str] = &[
    "variousartists",
    "various",
    "va",
    "unknownartist",
    "unknown",
];

/// Lowercases and strips diacritics so "Beyoncé" and "beyonce" compare equal.
pub(crate) fn fold(text: &str) -> String {
    // Libraries are mostly ASCII, and this runs over every row when matching.
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    text.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Compilation albums file under this one artist, as in iTunes.
pub(crate) const COMPILATIONS_GROUP: &str = "compilations";

/// Which artist an album files under: "Compilations" for compilations,
/// otherwise the album artist, falling back to the track artist. Migration 3
/// repeats this in SQL for rows that predate the column.
pub(crate) fn artist_group(album_artist: &str, artist: &str, compilation: bool) -> String {
    if compilation {
        COMPILATIONS_GROUP.to_owned()
    } else {
        fold(if album_artist.is_empty() {
            artist
        } else {
            album_artist
        })
    }
}

pub(crate) fn album_group(artist_group: &str, album: &str) -> String {
    format!("{artist_group}\u{1f}{}", fold(album))
}

/// Sort key for a name-like field. An explicit sort tag is taken verbatim
/// (it already encodes the user's intent); otherwise a leading English
/// article is dropped. Empty values yield `None` so SQLite's `NULLS LAST`
/// puts them at the end.
pub(crate) fn name_key(sort_tag: Option<&str>, value: &str) -> Option<String> {
    // Imported rows may carry a blank sort field; it must not hide the name.
    let sort_tag = sort_tag.filter(|tag| !tag.trim().is_empty());
    let folded = fold(sort_tag.unwrap_or(value));
    let folded = folded.trim();
    let key = match sort_tag {
        Some(_) => folded,
        None => strip_article(folded),
    };
    (!key.is_empty()).then(|| key.to_owned())
}

/// Sort key for a field that never has articles (genre).
pub(crate) fn plain_key(value: &str) -> Option<String> {
    let folded = fold(value);
    let folded = folded.trim();
    (!folded.is_empty()).then(|| folded.to_owned())
}

/// Identity of a recording's title for matching across libraries: release
/// notes and featured artists are dropped, then only letters and digits
/// remain, so punctuation and spacing differences between taggers vanish.
pub(crate) fn title_key(title: &str) -> Option<String> {
    let tidy = tidy(title);
    let mut rest = tidy.as_str();
    // Only trailing decorations come off; one in the middle is part of the name.
    loop {
        let shorter = if let Some((head, group)) = trailing_group(rest)
            && (is_release_note(group) || is_feature(group))
        {
            Some(head)
        } else if let Some((head, suffix)) = rest.rsplit_once(" - ")
            && is_release_note(suffix)
        {
            Some(head)
        } else {
            before_marker(rest, FEATURE_MARKERS.iter())
        };
        match shorter.map(str::trim_end) {
            Some(head) if !head.is_empty() => rest = head,
            _ => break,
        }
    }
    alphanumeric(&rest.replace('&', " and "))
}

/// Identity of the primary artist: everything before the first featuring or
/// co-artist marker, without a leading article.
pub(crate) fn artist_key(artist: &str) -> Option<String> {
    // Brackets are flattened so "(feat. X)" is found like a bare "feat. X".
    let tidy = tidy(&artist.replace(['(', ')', '[', ']'], " "));
    let primary =
        before_marker(&tidy, FEATURE_MARKERS.iter().chain(CO_ARTIST_MARKERS)).unwrap_or(&tidy);
    alphanumeric(strip_article(primary.trim()))
        .filter(|key| !ANONYMOUS_ARTISTS.contains(&key.as_str()))
}

fn tidy(text: &str) -> String {
    fold(text)
        .replace(['\u{2013}', '\u{2014}', '\u{2015}'], "-")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Splits `"Song (Live)"` into `("Song ", "Live")`.
fn trailing_group(text: &str) -> Option<(&str, &str)> {
    let (close, open) = match text.chars().next_back()? {
        ')' => (')', '('),
        ']' => (']', '['),
        _ => return None,
    };
    let mut depth = 0;
    for (index, c) in text.char_indices().rev() {
        if c == close {
            depth += 1;
        } else if c == open {
            depth -= 1;
            if depth == 0 {
                return Some((&text[..index], &text[index + 1..text.len() - 1]));
            }
        }
    }
    None
}

/// A release note, ignoring any year: "Remastered 2009" and "2011 Remaster".
fn is_release_note(text: &str) -> bool {
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty() && !is_year(word))
        .collect();
    RELEASE_NOTES.contains(&words.join(" ").as_str())
}

fn is_year(word: &str) -> bool {
    word.len() == 4
        && (word.starts_with("19") || word.starts_with("20"))
        && word.bytes().all(|b| b.is_ascii_digit())
}

fn is_feature(text: &str) -> bool {
    FEATURE_PREFIXES
        .iter()
        .any(|prefix| text.trim_start().starts_with(prefix))
}

fn before_marker<'a, 'm>(
    text: &'a str,
    markers: impl Iterator<Item = &'m &'m str>,
) -> Option<&'a str> {
    markers
        .filter_map(|marker| text.find(marker))
        .min()
        .map(|end| &text[..end])
}

/// Falls back to the trimmed text for names with no letters or digits ("!!!").
fn alphanumeric(text: &str) -> Option<String> {
    let key: String = text.chars().filter(|c| c.is_alphanumeric()).collect();
    if key.is_empty() {
        Some(text.trim().to_owned()).filter(|t| !t.is_empty())
    } else {
        Some(key)
    }
}

fn strip_article(folded: &str) -> &str {
    ARTICLES
        .iter()
        .find_map(|article| folded.strip_prefix(article))
        .map(str::trim_start)
        .filter(|rest| !rest.is_empty())
        .unwrap_or(folded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_strips_case_and_diacritics() {
        assert_eq!(fold("Beyoncé"), "beyonce");
        assert_eq!(fold("Mötley Crüe"), "motley crue");
    }

    #[test]
    fn articles_are_stripped_only_as_whole_leading_words() {
        assert_eq!(name_key(None, "The Beatles").as_deref(), Some("beatles"));
        assert_eq!(
            name_key(None, "A Night at the Opera").as_deref(),
            Some("night at the opera")
        );
        assert_eq!(
            name_key(None, "An Ordinary Day").as_deref(),
            Some("ordinary day")
        );
        assert_eq!(
            name_key(None, "Another Day").as_deref(),
            Some("another day")
        );
        assert_eq!(name_key(None, "Theater").as_deref(), Some("theater"));
    }

    #[test]
    fn a_name_that_is_only_an_article_is_kept() {
        assert_eq!(name_key(None, "The").as_deref(), Some("the"));
        assert_eq!(name_key(None, "A ").as_deref(), Some("a"));
    }

    #[test]
    fn explicit_sort_tag_wins_and_keeps_its_article() {
        assert_eq!(
            name_key(Some("The Beatles"), "Beatles, The").as_deref(),
            Some("the beatles")
        );
    }

    #[test]
    fn blank_sort_tag_falls_back_to_the_name() {
        assert_eq!(
            name_key(Some(" "), "The Beatles").as_deref(),
            Some("beatles")
        );
    }

    #[test]
    fn empty_values_have_no_key() {
        assert_eq!(name_key(None, "  "), None);
        assert_eq!(plain_key(""), None);
    }

    fn same_title(a: &str, b: &str) -> bool {
        let (a, b) = (title_key(a), title_key(b));
        a.is_some() && a == b
    }

    #[test]
    fn release_notes_do_not_change_the_title() {
        let variants = [
            "Come Together",
            "Come Together - Remastered 2009",
            "Come Together (Remastered 2009)",
            "Come Together [2009 Remaster]",
            "Come Together (Remastered)",
            "Come Together - 2019 Remaster",
            "Come Together (Mono)",
            "Come Together (Stereo Version)",
            "Come Together - Single Version",
            "Come Together (Album Version)",
            "Come Together (Explicit)",
            "Come Together [Clean]",
            "Come Together (Radio Edit)",
            "Come Together (Bonus Track)",
            "Come Together (Deluxe Edition)",
            "Come Together - Deluxe",
            "Come Together (Remastered 2009) [Explicit]",
            "Come Together - Remastered 2009 - Mono",
            "COME TOGETHER",
        ];
        for variant in variants {
            assert!(same_title("Come Together", variant), "{variant}");
        }
    }

    #[test]
    fn featured_artists_do_not_change_the_title() {
        let variants = [
            "Song (feat. Someone)",
            "Song [ft. Someone Else]",
            "Song (featuring Someone)",
            "Song feat. Someone",
            "Song ft. Someone (Remastered 2011)",
            "Song (Feat. Someone) - Single Version",
        ];
        for variant in variants {
            assert!(same_title("Song", variant), "{variant}");
        }
    }

    #[test]
    fn different_recordings_keep_their_titles_apart() {
        let recordings = [
            "Song",
            "Song (Live)",
            "Song (Live at Wembley 1986)",
            "Song (Remix)",
            "Song (Dave Remix)",
            "Song (Acoustic)",
            "Song (Demo)",
            "Song (Instrumental)",
            "Song (Club Edit)",
            "Song (Edit)",
            "Song (Live) Part 2",
            "Song (1999)",
            "Song Reprise",
            "Songs",
        ];
        for (i, a) in recordings.iter().enumerate() {
            for b in &recordings[i + 1..] {
                assert!(!same_title(a, b), "{a} vs {b}");
            }
        }
    }

    #[test]
    fn a_kept_note_reads_the_same_in_brackets_or_after_a_dash() {
        assert!(same_title("Song (Live)", "Song - Live"));
    }

    #[test]
    fn a_decoration_stripped_from_a_stack_leaves_the_rest_intact() {
        assert!(same_title("Song (Live)", "Song (Live) (Remastered 2009)"));
        assert!(!same_title("Song", "Song (Live) (Remastered 2009)"));
        assert!(same_title("Song (Remix) [feat. X]", "Song (Remix)"));
    }

    #[test]
    fn punctuation_and_spacing_do_not_change_the_title() {
        assert!(same_title("Don\u{2019}t Stop", "Don't Stop"));
        assert!(same_title("Don't Stop", "Dont Stop"));
        assert!(same_title(
            "I Am\u{2026} Sasha Fierce",
            "I Am... Sasha Fierce"
        ));
        assert!(same_title("Rock & Roll", "Rock and Roll"));
        assert!(same_title("Mr. Brightside", "Mr Brightside"));
        assert!(same_title("Song \u{2013} Remastered 2009", "Song"));
        assert!(same_title("Beyonc\u{e9}", "BEYONCE"));
    }

    #[test]
    fn titles_made_only_of_notes_or_symbols_are_kept() {
        assert_eq!(title_key("(Remastered)").as_deref(), Some("remastered"));
        assert_eq!(title_key("!!!").as_deref(), Some("!!!"));
        assert_eq!(title_key("1999").as_deref(), Some("1999"));
        assert_eq!(title_key("  "), None);
        assert_eq!(title_key(""), None);
    }

    #[test]
    fn primary_artist_is_the_text_before_any_co_artist_marker() {
        let variants = [
            "Daft Punk",
            "Daft Punk feat. Pharrell Williams",
            "Daft Punk ft. Pharrell",
            "Daft Punk featuring Pharrell",
            "Daft Punk (feat. Pharrell Williams)",
            "Daft Punk [ft. Pharrell]",
            "Daft Punk & Pharrell Williams",
            "Daft Punk, Pharrell Williams",
            "Daft Punk x Pharrell",
            "Daft Punk with Pharrell",
            "DAFT PUNK",
        ];
        for variant in variants {
            assert_eq!(
                artist_key(variant).as_deref(),
                Some("daftpunk"),
                "{variant}"
            );
        }
    }

    #[test]
    fn artist_keys_ignore_articles_diacritics_and_punctuation() {
        assert_eq!(artist_key("The Beatles"), artist_key("Beatles"));
        assert_eq!(artist_key("Beyonc\u{e9}"), artist_key("Beyonce"));
        assert_eq!(artist_key("AC/DC"), artist_key("ACDC"));
        assert_eq!(artist_key("Jay-Z"), artist_key("Jay Z"));
        assert_ne!(artist_key("Queen"), artist_key("Queens of the Stone Age"));
        assert_ne!(artist_key("Extreme"), artist_key("X"));
    }

    #[test]
    fn placeholder_artists_have_no_key() {
        for placeholder in [
            "Various Artists",
            "various",
            "VA",
            "Unknown Artist",
            "",
            "  ",
        ] {
            assert_eq!(artist_key(placeholder), None, "{placeholder:?}");
        }
    }
}
