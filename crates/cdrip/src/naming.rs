//! Turning metadata into file system paths that every platform accepts.

use std::path::{Path, PathBuf};

/// File names may not exceed 255 bytes on common file systems; leave room for the track prefix and
/// extension.
const MAX_COMPONENT_BYTES: usize = 180;
/// Names Windows reserves for devices, with or without an extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Makes one path component safe: separators and characters Windows forbids become `_`, as do
/// control characters; leading dots (hidden files) and trailing dots and spaces (which Windows
/// silently drops) are removed; device names are prefixed; the result is never empty.
pub(crate) fn sanitize(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut clean = replaced
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_owned();
    if clean.len() > MAX_COMPONENT_BYTES {
        let mut end = MAX_COMPONENT_BYTES;
        while !clean.is_char_boundary(end) {
            end -= 1;
        }
        clean.truncate(end);
        clean.truncate(clean.trim_end().len());
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

/// `<dest>/<album artist>/<album>/<disc>-<track> <title>.<extension>`.
pub(crate) fn track_path(
    dest: &Path,
    album_artist: &str,
    album: &str,
    disc: u32,
    track: u32,
    title: &str,
    extension: &str,
) -> PathBuf {
    dest.join(sanitize(album_artist))
        .join(sanitize(album))
        .join(format!("{disc}-{track:02} {}.{extension}", sanitize(title)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_characters_become_underscores() {
        assert_eq!(sanitize("AC/DC"), "AC_DC");
        assert_eq!(
            sanitize("What?: A \"Test\" <1> | *"),
            "What__ A _Test_ _1_ _ _"
        );
        assert_eq!(sanitize("back\\slash"), "back_slash");
        assert_eq!(sanitize("tab\there\u{7}bell"), "tab_here_bell");
    }

    #[test]
    fn leading_dots_and_trailing_dots_and_spaces_go() {
        assert_eq!(sanitize(".hidden"), "hidden");
        assert_eq!(sanitize("..."), "_");
        assert_eq!(sanitize("Ellipsis... "), "Ellipsis");
        assert_eq!(sanitize("  padded  "), "padded");
        assert_eq!(sanitize(""), "_");
        assert_eq!(sanitize("../.."), "_");
    }

    #[test]
    fn windows_device_names_are_prefixed() {
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("nul"), "_nul");
        assert_eq!(sanitize("Lpt1.txt"), "_Lpt1.txt");
        assert_eq!(sanitize("Console"), "Console");
        assert_eq!(sanitize("COM10"), "COM10");
    }

    #[test]
    fn unicode_is_kept_and_long_names_are_cut_on_a_character_boundary() {
        assert_eq!(
            sanitize("Sigur R\u{f3}s \u{2014} \u{FF08}\u{FF09}"),
            "Sigur R\u{f3}s \u{2014} \u{FF08}\u{FF09}"
        );
        let long = "\u{3042}".repeat(200);
        let cut = sanitize(&long);
        assert!(cut.len() <= MAX_COMPONENT_BYTES);
        assert!(cut.chars().all(|c| c == '\u{3042}'));
        assert_eq!(sanitize(&"a".repeat(500)).len(), MAX_COMPONENT_BYTES);
    }

    #[test]
    fn track_paths_follow_the_documented_layout() {
        let path = track_path(
            Path::new("/music"),
            "AC/DC",
            "Back in Black",
            1,
            3,
            "Hells Bells",
            "flac",
        );
        assert_eq!(
            path,
            Path::new("/music/AC_DC/Back in Black/1-03 Hells Bells.flac")
        );
        let path = track_path(
            Path::new("/music"),
            "Pink Floyd",
            "The Wall",
            2,
            12,
            "Outside the Wall?",
            "m4a",
        );
        assert_eq!(
            path,
            Path::new("/music/Pink Floyd/The Wall/2-12 Outside the Wall_.m4a")
        );
    }

    #[test]
    fn a_title_cannot_escape_its_directory() {
        let path = track_path(
            Path::new("/music"),
            "A",
            "B",
            1,
            1,
            "../../etc/passwd",
            "flac",
        );
        assert_eq!(path.parent().unwrap(), Path::new("/music/A/B"));
    }
}
