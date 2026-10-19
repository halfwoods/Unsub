//! What counts as audio, artwork, junk or an unfinished download, judged by name alone.

const AUDIO: &[&str] = &[
    "flac", "mp3", "m4a", "ogg", "opus", "wav", "aif", "aiff", "wv",
];
const ARTWORK: &[&str] = &["jpg", "jpeg", "png", "gif", "webp"];
const IN_PROGRESS: &[&str] = &["crdownload", "part", "download", "partial"];

pub(crate) fn extension(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    Some(extension.to_ascii_lowercase())
}

fn has_extension(name: &str, list: &[&str]) -> bool {
    extension(name).is_some_and(|extension| list.contains(&extension.as_str()))
}

pub(crate) fn is_audio(name: &str) -> bool {
    has_extension(name, AUDIO)
}

pub(crate) fn is_artwork(name: &str) -> bool {
    has_extension(name, ARTWORK)
}

/// Browsers rename a download to its real name when it finishes; until then it carries one of
/// these suffixes (Safari's is a folder).
pub(crate) fn is_in_progress(name: &str) -> bool {
    has_extension(name, IN_PROGRESS)
}

/// Clutter that archivers and file managers add and that no one wants in the library.
pub(crate) fn is_junk(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or_default();
    path.split(['/', '\\']).any(|part| part == "__MACOSX")
        || name.starts_with("._")
        || name.eq_ignore_ascii_case(".DS_Store")
        || name.eq_ignore_ascii_case("Thumbs.db")
        || name.eq_ignore_ascii_case("desktop.ini")
}
