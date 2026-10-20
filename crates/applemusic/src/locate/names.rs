//! How Music names the folders and files it creates, and the looser comparisons for names that
//! do not follow those rules. The rules were read off a real 37,000-song library, not documented
//! anywhere by Apple.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Replaced by `_` in every name Music writes: the characters filesystems refuse, and the curly
/// quotes (never found in a name in the real library, though common in titles). Control
/// characters go too.
const REPLACED: [char; 15] = [
    '/', '\\', ':', '*', '?', '"', '<', '>', '|', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201C}',
    '\u{201D}', '\u{00B4}',
];

/// The folder Music makes for an artist or an album. Whatever is left of a blank name is the
/// `fallback`.
pub(crate) fn folder_name(name: &str, fallback: &str) -> String {
    cleaned(name, true).unwrap_or_else(|| fallback.to_owned())
}

/// A song's file name without its extension: track number, then title. A trailing period stays,
/// because the extension follows it; in a folder name it would be refused.
pub(crate) fn track_stems(title: &str, track: Option<u32>, disc: Option<u32>) -> Vec<String> {
    let title = cleaned(title, false).unwrap_or_else(|| "Untitled".to_owned());
    let mut stems = Vec::new();
    if let Some(track) = track.filter(|track| *track > 0) {
        let with_disc = disc
            .filter(|disc| *disc > 0)
            .map(|disc| format!("{disc}-{track:02} {title}"));
        let plain = format!("{track:02} {title}");
        // A second disc is where the disc prefix is certain; on a first disc either may be used.
        match (with_disc, disc) {
            (Some(with_disc), Some(disc)) if disc > 1 => stems.extend([with_disc, plain]),
            (Some(with_disc), _) => stems.extend([plain, with_disc]),
            (None, _) => stems.push(plain),
        }
    }
    stems.push(title);
    stems
}

/// `None` for a blank name. Edges: one leading space or period becomes `_` so the name is not
/// hidden or padded, trailing spaces are dropped, and in folders a trailing period becomes `_`.
fn cleaned(name: &str, replace_trailing_period: bool) -> Option<String> {
    let mut cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || REPLACED.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    if cleaned.starts_with([' ', '.']) {
        cleaned.replace_range(..1, "_");
    }
    let trimmed = cleaned.trim_end().len();
    cleaned.truncate(trimmed);
    if replace_trailing_period && cleaned.ends_with('.') {
        cleaned.pop();
        cleaned.push('_');
    }
    Some(cleaned).filter(|cleaned| !cleaned.is_empty())
}

/// For comparing names that must be the same folder or file: composed, and without case. APFS
/// keeps whatever form a name was written in, and older folders are decomposed.
pub(crate) fn fold(name: &str) -> String {
    name.nfc().flat_map(char::to_lowercase).collect()
}

/// For comparing names that mean the same thing but were spelled differently: no case, accents,
/// spaces or punctuation. `Wait... (Live)` and `wait live` are the same.
pub(crate) fn loose(name: &str) -> String {
    name.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// The leading track number of a file name, as other tools and older iTunes wrote it: `01 Title`,
/// `1-01 Title`, `07. Title`, `13- Title`, `08_Title`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Numbered<'a> {
    pub disc: Option<u32>,
    pub track: u32,
    pub title: &'a str,
}

pub(crate) fn split_number(stem: &str) -> Option<Numbered<'_>> {
    let (first, after_first) = number(stem)?;
    // "1-01 Title" is disc and track; "13- Title" and "01 - Title" are a track and a separator.
    let (disc, track, rest) = match after_first.strip_prefix('-').and_then(number) {
        Some((second, rest)) if !after_first.starts_with("- ") => (Some(first), second, rest),
        _ => (None, first, after_first),
    };
    let title = rest.trim_start_matches([' ', '.', '-', '_', ')']);
    // A number needs something after it to be a prefix, and a separator to end it.
    let separated = rest.starts_with([' ', '.', '-', '_', ')']);
    (separated && !title.is_empty()).then_some(Numbered { disc, track, title })
}

/// A run of one to three digits, and what follows it.
fn number(text: &str) -> Option<(u32, &str)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let (number, rest) = text.split_at(digits);
    Some((number.parse().ok()?, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_what_music_replaces() {
        assert_eq!(folder_name("AC/DC", "x"), "AC_DC");
        assert_eq!(folder_name("What?", "x"), "What_");
        assert_eq!(
            folder_name("Doctor Who: Series 1", "x"),
            "Doctor Who_ Series 1"
        );
        assert_eq!(
            folder_name("Say \"Hi\" <now> | * \\", "x"),
            "Say _Hi_ _now_ _ _ _"
        );
        assert_eq!(folder_name("Promo CD - \tHOSP", "x"), "Promo CD - _HOSP");
        // Curly quotes and the acute accent used as an apostrophe.
        assert_eq!(
            folder_name("Jeff\u{2019}s \u{201C}Mix\u{201D} \u{00B4}", "x"),
            "Jeff_s _Mix_ _"
        );
    }

    #[test]
    fn keeps_what_music_keeps() {
        assert_eq!(
            folder_name("Beyoncé … ™ （live）", "x"),
            "Beyoncé … ™ （live）"
        );
        assert_eq!(
            folder_name("Ünïcode & (Parens) [Brackets]", "x"),
            "Ünïcode & (Parens) [Brackets]"
        );
    }

    #[test]
    fn edges_follow_the_real_library() {
        // From folders found next to the songs they were made for.
        assert_eq!(
            folder_name("...I Care Because You Do", "x"),
            "_..I Care Because You Do"
        );
        assert_eq!(
            folder_name(" CHAMPIONS (INSTRUMENTAL)    ", "x"),
            "_CHAMPIONS (INSTRUMENTAL)"
        );
        assert_eq!(folder_name("Weekend In L.A.", "x"), "Weekend In L.A_");
        assert_eq!(folder_name("Big K.R.I.T.", "x"), "Big K.R.I.T_");
        assert_eq!(folder_name("Fever Ray ", "x"), "Fever Ray");
        assert_eq!(folder_name(" ", "x"), "_");
    }

    #[test]
    fn blank_names_take_the_fallback() {
        assert_eq!(folder_name("", "Unknown Album"), "Unknown Album");
        assert_eq!(track_stems("", None, None), ["Untitled"]);
    }

    #[test]
    fn file_names_are_number_then_title() {
        assert_eq!(
            track_stems("Airbag", Some(1), None),
            ["01 Airbag", "Airbag"]
        );
        assert_eq!(
            track_stems("Airbag", Some(12), Some(1)),
            ["12 Airbag", "1-12 Airbag", "Airbag"]
        );
        assert_eq!(
            track_stems("Airbag", Some(3), Some(2)),
            ["2-03 Airbag", "03 Airbag", "Airbag"]
        );
        assert_eq!(
            track_stems("Airbag", Some(123), None),
            ["123 Airbag", "Airbag"]
        );
        assert_eq!(track_stems("Airbag", None, Some(2)), ["Airbag"]);
        assert_eq!(track_stems("Airbag", Some(0), None), ["Airbag"]);
    }

    #[test]
    fn a_trailing_period_stays_in_a_file_name() {
        assert_eq!(track_stems("End.", Some(15), None), ["15 End.", "End."]);
        assert_eq!(track_stems("What?", Some(2), None), ["02 What_", "What_"]);
        assert_eq!(track_stems(" Se Quiser", None, None), ["_Se Quiser"]);
    }

    #[test]
    fn folds_case_and_unicode_form() {
        let composed = "Bj\u{00F6}rk";
        let decomposed = "Bjo\u{0308}rk";
        assert_ne!(composed, decomposed);
        assert_eq!(fold(composed), fold(decomposed));
        assert_eq!(fold("The BLACK Keys"), fold("the black keys"));
    }

    #[test]
    fn loose_names_ignore_accents_spaces_and_punctuation() {
        assert_eq!(loose("Wait... (Live)"), loose("wait live"));
        assert_eq!(loose("Páginas da Vida"), loose("paginas_da_vida"));
        assert_eq!(loose("  !?  "), "");
    }

    #[test]
    fn reads_track_numbers_as_tools_write_them() {
        let numbered = |stem| split_number(stem).map(|n| (n.disc, n.track, n.title.to_owned()));
        assert_eq!(numbered("01 Airbag"), Some((None, 1, "Airbag".into())));
        assert_eq!(numbered("1-01 Airbag"), Some((Some(1), 1, "Airbag".into())));
        assert_eq!(numbered("07. my baby"), Some((None, 7, "my baby".into())));
        assert_eq!(
            numbered("13- serenade"),
            Some((None, 13, "serenade".into()))
        );
        assert_eq!(
            numbered("05 - a foggy day"),
            Some((None, 5, "a foggy day".into()))
        );
        assert_eq!(
            numbered("08_frankie - marlena"),
            Some((None, 8, "frankie - marlena".into()))
        );
        assert_eq!(
            numbered("17-(don't worry)"),
            Some((None, 17, "(don't worry)".into()))
        );
        assert_eq!(numbered("2-26 jack"), Some((Some(2), 26, "jack".into())));
    }

    #[test]
    fn a_number_that_is_the_whole_title_is_not_a_prefix() {
        assert_eq!(split_number("1979"), None);
        assert_eq!(split_number("2Pac"), None);
        assert_eq!(split_number("1234 Song"), None);
        assert_eq!(split_number("Airbag"), None);
        assert_eq!(split_number("01 "), None);
    }
}
