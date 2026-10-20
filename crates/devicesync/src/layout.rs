//! Turning track metadata into names every FAT, exFAT and player firmware accepts.

use std::collections::HashSet;

use crate::{Error, Profile, Result, SyncItem};

/// FAT file names hold at most 255 UTF-16 units and some players limit the whole path to 255, so
/// each part is kept short enough for `Music/<artist>/<album>/<file>.ext` to fit.
const MAX_DIR_UNITS: usize = 64;
const MAX_FILE_UNITS: usize = 96;
/// Names Windows reserves for devices; a stick holding one cannot be read there.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A parsed path template such as `{album_artist}/{album}/{disc}-{track} {title}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Template(Vec<Vec<Part>>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Field(Field),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    AlbumArtist,
    Album,
    Title,
    Disc,
    Track,
}

impl Template {
    pub fn parse(template: &str) -> Result<Self> {
        let invalid =
            |detail: String| Error::InvalidProfile(format!("layout {template:?}: {detail}"));
        let segments = template
            .split('/')
            .map(|segment| {
                if segment.is_empty() {
                    return Err(invalid("empty folder or file name".to_owned()));
                }
                let mut parts = Vec::new();
                let mut rest = segment;
                while !rest.is_empty() {
                    match rest.find(['{', '}']) {
                        None => {
                            parts.push(Part::Text(rest.to_owned()));
                            rest = "";
                        }
                        Some(at) if rest[at..].starts_with('}') => {
                            return Err(invalid("unmatched }".to_owned()));
                        }
                        Some(at) => {
                            if at > 0 {
                                parts.push(Part::Text(rest[..at].to_owned()));
                            }
                            let close = rest[at..]
                                .find('}')
                                .ok_or_else(|| invalid("unmatched {".to_owned()))?
                                + at;
                            let name = &rest[at + 1..close];
                            let field = match name {
                                "album_artist" => Field::AlbumArtist,
                                "album" => Field::Album,
                                "title" => Field::Title,
                                "disc" => Field::Disc,
                                "track" => Field::Track,
                                other => {
                                    return Err(invalid(format!(
                                        "unknown placeholder {{{other}}}"
                                    )));
                                }
                            };
                            parts.push(Part::Field(field));
                            rest = &rest[close + 1..];
                        }
                    }
                }
                Ok(parts)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self(segments))
    }

    /// The device path of `item` below the profile's music folder, with `extension` added.
    pub fn path(&self, profile: &Profile, item: &SyncItem, extension: &str) -> String {
        let last = self.0.len() - 1;
        let mut path = profile.music_dir();
        for (index, segment) in self.0.iter().enumerate() {
            let text: String = segment
                .iter()
                .map(|part| match part {
                    Part::Text(text) => text.clone(),
                    Part::Field(field) => field_value(*field, item),
                })
                .collect();
            path.push('/');
            if index == last {
                path.push_str(&sanitise(&text, MAX_FILE_UNITS));
                path.push('.');
                path.push_str(extension);
            } else {
                path.push_str(&sanitise(&text, MAX_DIR_UNITS));
            }
        }
        path
    }
}

fn field_value(field: Field, item: &SyncItem) -> String {
    let or = |text: &str, fallback: &str| {
        if text.trim().is_empty() {
            fallback.to_owned()
        } else {
            text.to_owned()
        }
    };
    match field {
        Field::AlbumArtist => or(&item.album_artist, "Unknown Artist"),
        Field::Album => or(&item.album, "Unknown Album"),
        Field::Title => or(&item.title, "Untitled"),
        Field::Disc => item.disc.unwrap_or(1).to_string(),
        // Two digits keep tracks in order in a player's file browser; 00 sorts an unnumbered one first.
        Field::Track => format!("{:02}", item.track.unwrap_or(0)),
    }
}

/// One path component made safe for FAT and exFAT: forbidden characters and control characters
/// become `_`, leading dots (hidden files) and trailing dots and spaces (which Windows drops) are
/// removed, device names are prefixed, and the length is capped at `max_units` UTF-16 units on a
/// character boundary. Never empty.
pub(crate) fn sanitise(name: &str, max_units: usize) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut clean = replaced
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_owned();
    let mut units = 0;
    let end = clean
        .char_indices()
        .find(|(_, c)| {
            units += c.len_utf16();
            units > max_units
        })
        .map(|(at, _)| at);
    if let Some(end) = end {
        clean.truncate(end);
        clean.truncate(
            clean
                .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
                .len(),
        );
    }
    let stem = clean.split('.').next().unwrap_or_default();
    if clean.is_empty() {
        "_".to_owned()
    } else if RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        format!("_{clean}")
    } else {
        clean
    }
}

/// A playlist's file name (without extension) from its title.
pub(crate) fn playlist_name(title: &str) -> String {
    sanitise(title, MAX_FILE_UNITS)
}

/// `path` and `base` are the same destination but for a collision suffix: ` (2)` before the
/// extension. A file keeps its suffixed name for as long as it still derives from the same base, so
/// a collision that has since gone away does not make every later sync move the file.
pub(crate) fn same_destination(path: &str, base: &str) -> bool {
    let (path_stem, path_ext) = split_extension(path);
    let (base_stem, base_ext) = split_extension(base);
    if !path_ext.eq_ignore_ascii_case(base_ext) {
        return false;
    }
    if path_stem.eq_ignore_ascii_case(base_stem) {
        return true;
    }
    path_stem
        .get(..base_stem.len())
        .zip(path_stem.get(base_stem.len()..))
        .is_some_and(|(head, tail)| {
            head.eq_ignore_ascii_case(base_stem)
                && tail
                    .strip_prefix(" (")
                    .and_then(|rest| rest.strip_suffix(')'))
                    .is_some_and(|number| {
                        !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                    })
        })
}

/// `base`, or the first of `base (2)`, `base (3)`, ... that `taken` does not claim. FAT is
/// case-insensitive, so `taken` is asked for lower-case paths.
pub(crate) fn first_free(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let (stem, extension) = split_extension(base);
    let with_extension = |stem: &str| {
        if extension.is_empty() {
            stem.to_owned()
        } else {
            format!("{stem}.{extension}")
        }
    };
    std::iter::once(base.to_owned())
        .chain((2..).map(|n| with_extension(&format!("{stem} ({n})"))))
        .find(|candidate| !taken(&candidate.to_lowercase()))
        .unwrap_or_else(|| base.to_owned())
}

/// Splits `dir/name.ext` into `dir/name` and `ext`; no extension gives an empty one.
pub(crate) fn split_extension(path: &str) -> (&str, &str) {
    let name_start = path.rfind('/').map_or(0, |at| at + 1);
    match path[name_start..].rfind('.') {
        Some(dot) if dot > 0 => (&path[..name_start + dot], &path[name_start + dot + 1..]),
        _ => (path, ""),
    }
}

/// The paths in `taken` as a set for case-insensitive lookups.
pub(crate) fn lowercase_set<'a>(paths: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
    paths.into_iter().map(str::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Profile;

    fn item(
        artist: &str,
        album: &str,
        title: &str,
        disc: Option<u32>,
        track: Option<u32>,
    ) -> SyncItem {
        SyncItem {
            library_id: 1,
            source: "/library/x.flac".into(),
            album_artist: artist.to_owned(),
            album: album.to_owned(),
            title: title.to_owned(),
            disc,
            track,
            size: 1,
            mtime: 1,
        }
    }

    #[test]
    fn the_default_layout_gives_artist_album_disc_track_title() {
        let template = Template::parse(&Profile::default().layout).unwrap();
        let path = template.path(
            &Profile::default(),
            &item("The Beatles", "Abbey Road", "Something", Some(1), Some(2)),
            "flac",
        );
        assert_eq!(path, "Music/The Beatles/Abbey Road/1-02 Something.flac");
    }

    #[test]
    fn missing_metadata_gets_placeholders() {
        let template = Template::parse(&Profile::default().layout).unwrap();
        let path = template.path(&Profile::default(), &item("", "  ", "", None, None), "mp3");
        assert_eq!(path, "Music/Unknown Artist/Unknown Album/1-00 Untitled.mp3");
    }

    #[test]
    fn custom_layouts_and_music_dirs_apply() {
        let profile = Profile {
            music_dir: "/Audio/Tunes/".to_owned(),
            layout: "{album} - {title}".to_owned(),
            ..Profile::default()
        };
        let template = Template::parse(&profile.layout).unwrap();
        assert_eq!(
            template.path(&profile, &item("A", "B", "C", None, None), "ogg"),
            "Audio/Tunes/B - C.ogg"
        );
    }

    #[test]
    fn bad_templates_are_refused() {
        for bad in [
            "",
            "{title",
            "title}",
            "{nope}",
            "{title}//{album}",
            "/{title}",
            "{title}/",
        ] {
            assert!(
                matches!(Template::parse(bad), Err(Error::InvalidProfile(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_field_cannot_add_folders_or_escape() {
        let template = Template::parse(&Profile::default().layout).unwrap();
        let path = template.path(
            &Profile::default(),
            &item("../../AC/DC", "a\\b", "x:y", Some(1), Some(1)),
            "flac",
        );
        assert_eq!(path, "Music/_.._AC_DC/a_b/1-01 x_y.flac");
        assert_eq!(path.split('/').count(), 4);
    }

    #[test]
    fn forbidden_characters_dots_and_spaces() {
        assert_eq!(
            sanitise("What?: A \"Test\" <1> | *", 64),
            "What__ A _Test_ _1_ _ _"
        );
        assert_eq!(sanitise("AC/DC", 64), "AC_DC");
        assert_eq!(sanitise("tab\there\u{7}bell", 64), "tab_here_bell");
        assert_eq!(sanitise(".hidden", 64), "hidden");
        assert_eq!(sanitise("Ellipsis... ", 64), "Ellipsis");
        assert_eq!(sanitise("   ", 64), "_");
        assert_eq!(sanitise("...", 64), "_");
    }

    #[test]
    fn device_names_are_prefixed() {
        assert_eq!(sanitise("CON", 64), "_CON");
        assert_eq!(sanitise("aux.txt", 64), "_aux.txt");
        assert_eq!(sanitise("Console", 64), "Console");
    }

    #[test]
    fn length_is_capped_in_utf16_units_on_a_character_boundary() {
        assert_eq!(sanitise(&"a".repeat(300), 64).len(), 64);
        let cut = sanitise(&"\u{3042}".repeat(100), 64);
        assert_eq!(cut.chars().count(), 64);
        // A character outside the BMP is two units, so only 32 fit.
        let cut = sanitise(&"\u{1F3B5}".repeat(100), 64);
        assert_eq!(cut.chars().count(), 32);
        // Trailing space left by the cut is trimmed.
        assert_eq!(
            sanitise(&format!("{} tail", "a".repeat(63)), 64),
            "a".repeat(63)
        );
    }

    #[test]
    fn a_long_path_stays_under_the_usual_limit() {
        let long = "x".repeat(400);
        let template = Template::parse(&Profile::default().layout).unwrap();
        let path = template.path(
            &Profile::default(),
            &item(&long, &long, &long, Some(1), Some(1)),
            "flac",
        );
        assert!(path.len() <= 255, "{}", path.len());
    }

    #[test]
    fn collision_suffixes_are_deterministic_and_case_insensitive() {
        let taken = lowercase_set(["Music/A/Song.flac", "music/a/song (2).flac"]);
        let free = |base: &str| first_free(base, &|candidate| taken.contains(candidate));
        assert_eq!(free("Music/A/Song.flac"), "Music/A/Song (3).flac");
        assert_eq!(free("Music/A/Other.flac"), "Music/A/Other.flac");
        assert_eq!(
            first_free("Music/A/NoExt", &|c| c == "music/a/noext"),
            "Music/A/NoExt (2)"
        );
    }

    #[test]
    fn a_suffixed_path_still_belongs_to_its_base() {
        assert!(same_destination("Music/A/Song.flac", "Music/A/Song.flac"));
        assert!(same_destination(
            "Music/A/Song (2).flac",
            "Music/A/Song.flac"
        ));
        assert!(same_destination(
            "Music/A/Song (17).flac",
            "Music/A/Song.flac"
        ));
        assert!(same_destination("MUSIC/a/song.FLAC", "Music/A/Song.flac"));
        assert!(!same_destination(
            "Music/A/Song (x).flac",
            "Music/A/Song.flac"
        ));
        assert!(!same_destination(
            "Music/A/Song ().flac",
            "Music/A/Song.flac"
        ));
        assert!(!same_destination(
            "Music/A/Song (2).mp3",
            "Music/A/Song.flac"
        ));
        assert!(!same_destination("Music/B/Song.flac", "Music/A/Song.flac"));
        assert!(!same_destination(
            "Music/A/Song 2.flac",
            "Music/A/Song.flac"
        ));
    }

    #[test]
    fn extension_splitting_ignores_dots_in_folders() {
        assert_eq!(split_extension("a.b/c"), ("a.b/c", ""));
        assert_eq!(split_extension("a.b/c.flac"), ("a.b/c", "flac"));
        assert_eq!(split_extension("a/.hidden"), ("a/.hidden", ""));
    }
}
