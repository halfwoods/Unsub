//! Working out where a download came from, and what album it is, from its names alone. Reading
//! tags is the fallback when these are not enough.

use crate::model::Store;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Guess {
    pub store: Store,
    pub artist: Option<String>,
    pub album: Option<String>,
}

const UNKNOWN_ARTIST: &str = "Unknown Artist";

/// `stem` is the zip's file name without ".zip", or the folder's name; `audio` holds the paths
/// of the audio inside, relative to it.
pub(crate) fn from_names(stem: &str, audio: &[String]) -> Guess {
    // Browsers number repeat downloads "Album (1).zip" while the files inside keep the real name.
    let stem = strip_copy_suffix(stem);
    if let Some((artist, album)) = bandcamp(stem, audio) {
        return Guess {
            store: Store::Bandcamp,
            artist: Some(artist),
            album: Some(album),
        };
    }
    if let Some((artist, album)) = qobuz(stem) {
        return Guess {
            store: Store::Qobuz,
            artist,
            album,
        };
    }
    let names = std::iter::once(stem).chain(audio.iter().map(String::as_str));
    let store = names
        .map(store_keyword)
        .find(|store| *store != Store::Unknown)
        .unwrap_or(Store::Unknown);
    Guess {
        store,
        artist: None,
        album: None,
    }
}

/// Stores that put their own name in a file name, a folder name or a tag.
pub(crate) fn store_keyword(text: &str) -> Store {
    let text = text.to_ascii_lowercase();
    [
        ("bandcamp", Store::Bandcamp),
        ("qobuz", Store::Qobuz),
        ("7digital", Store::SevenDigital),
        ("hdtracks", Store::Hdtracks),
    ]
    .into_iter()
    .find(|(keyword, _)| text.contains(keyword))
    .map_or(Store::Unknown, |(_, store)| store)
}

/// The last resort when neither the store's naming nor the tags say who made the album.
pub(crate) fn fallback_names(stem: &str) -> (String, String) {
    let (artist, album) = split_artist_album(strip_copy_suffix(stem));
    match (artist, album) {
        (Some(artist), Some(album)) => (artist, album),
        _ => (UNKNOWN_ARTIST.to_owned(), stem.trim().to_owned()),
    }
}

/// Bandcamp names the zip "Artist - Album" and every track "Artist - Album - 01 Title".
fn bandcamp(stem: &str, audio: &[String]) -> Option<(String, String)> {
    let (artist, album) = stem.split_once(" - ")?;
    let prefix = format!("{stem} - ");
    let every_track_matches = !audio.is_empty()
        && audio.iter().all(|path| {
            file_stem(path)
                .strip_prefix(&prefix)
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        });
    every_track_matches.then(|| (artist.trim().to_owned(), album.trim().to_owned()))
}

/// Qobuz names the download "Artist - Album (Year) [FLAC] [24B-96kHz]". The quality tag is what
/// separates it from any other "Artist - Album" with brackets after it.
fn qobuz(stem: &str) -> Option<(Option<String>, Option<String>)> {
    let mut rest = stem.trim_end();
    let mut has_quality_tag = false;
    while rest.ends_with(']') {
        let open = rest.rfind('[')?;
        has_quality_tag |= is_quality_tag(&rest[open + 1..rest.len() - 1]);
        rest = rest[..open].trim_end();
    }
    has_quality_tag.then(|| split_artist_album(strip_year(rest)))
}

/// `24B-96kHz` or `16B-44.1kHz`.
fn is_quality_tag(tag: &str) -> bool {
    let tag = tag.to_ascii_lowercase();
    let Some((bits, rate)) = tag.split_once("b-") else {
        return false;
    };
    bits.parse::<u32>().is_ok()
        && rate
            .strip_suffix("khz")
            .is_some_and(|khz| khz.parse::<f32>().is_ok())
}

fn strip_year(name: &str) -> &str {
    if let Some(open) = name.rfind('(')
        && let Some(year) = name[open + 1..].strip_suffix(')')
        && year.len() == 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
    {
        return name[..open].trim_end();
    }
    name
}

fn strip_copy_suffix(name: &str) -> &str {
    if let Some(open) = name.rfind(" (")
        && let Some(number) = name[open + 2..].strip_suffix(')')
        && !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        // "(1997)" is a year, not a browser's copy number.
        && number.len() < 4
    {
        return &name[..open];
    }
    name
}

fn split_artist_album(name: &str) -> (Option<String>, Option<String>) {
    match name.split_once(" - ") {
        Some((artist, album)) if !artist.trim().is_empty() && !album.trim().is_empty() => (
            Some(artist.trim().to_owned()),
            Some(album.trim().to_owned()),
        ),
        _ => (None, None),
    }
}

/// The file name of a relative path, without its extension.
fn file_stem(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or_default();
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks(prefix: &str) -> Vec<String> {
        ["01 Airbag.flac", "02 Paranoid Android.flac"]
            .iter()
            .map(|track| format!("{prefix}{track}"))
            .collect()
    }

    #[test]
    fn recognizes_bandcamp_by_the_track_prefix() {
        let guess = from_names(
            "Radiohead - OK Computer",
            &tracks("Radiohead - OK Computer - "),
        );
        assert_eq!(
            guess,
            Guess {
                store: Store::Bandcamp,
                artist: Some("Radiohead".into()),
                album: Some("OK Computer".into()),
            }
        );
    }

    #[test]
    fn bandcamp_survives_a_browser_copy_suffix_and_a_folder_layer() {
        let audio = tracks("Radiohead - OK Computer/Radiohead - OK Computer - ");
        let guess = from_names("Radiohead - OK Computer (2)", &audio);
        assert_eq!(guess.store, Store::Bandcamp);
        assert_eq!(guess.album.as_deref(), Some("OK Computer"));
    }

    #[test]
    fn two_part_names_without_the_track_prefix_are_not_bandcamp() {
        let guess = from_names("Radiohead - OK Computer", &tracks(""));
        assert_eq!(guess.store, Store::Unknown);
        assert_eq!(guess.artist, None);
    }

    #[test]
    fn recognizes_qobuz_by_its_quality_tag() {
        let guess = from_names("Björk - Homogenic (1997) [FLAC] [24B-96kHz]", &tracks(""));
        assert_eq!(
            guess,
            Guess {
                store: Store::Qobuz,
                artist: Some("Björk".into()),
                album: Some("Homogenic".into()),
            }
        );
        let cd = from_names("Daft Punk - Discovery (2001) [FLAC] [16B-44.1kHz]", &[]);
        assert_eq!(cd.store, Store::Qobuz);
        assert_eq!(cd.album.as_deref(), Some("Discovery"));
    }

    #[test]
    fn brackets_without_a_quality_tag_are_not_qobuz() {
        assert_eq!(
            from_names("Artist - Album [Remastered]", &[]).store,
            Store::Unknown
        );
        assert_eq!(
            from_names("Artist - Album [FLAC]", &[]).store,
            Store::Unknown
        );
    }

    #[test]
    fn finds_stores_that_put_their_name_in_a_path() {
        assert_eq!(
            from_names("Boards of Canada - Geogaddi [7digital]", &[]).store,
            Store::SevenDigital
        );
        assert_eq!(
            from_names("Album", &["HDtracks/01 Song.flac".to_owned()]).store,
            Store::Hdtracks
        );
    }

    #[test]
    fn falls_back_to_a_generic_split_or_the_whole_name() {
        assert_eq!(
            fallback_names("Pink Floyd - The Wall (1)"),
            ("Pink Floyd".to_owned(), "The Wall".to_owned())
        );
        assert_eq!(
            fallback_names("Some Download"),
            ("Unknown Artist".to_owned(), "Some Download".to_owned())
        );
    }

    #[test]
    fn a_year_in_parentheses_is_not_a_copy_number() {
        assert_eq!(strip_copy_suffix("Album (1997)"), "Album (1997)");
        assert_eq!(strip_copy_suffix("Album (12)"), "Album");
    }
}
