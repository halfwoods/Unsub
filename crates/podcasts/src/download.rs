//! Episode downloads: a `<id>.part` file next to the final name, resumed with `Range`, renamed
//! into place once complete so a half-written file is never mistaken for an episode.
//!
//! The first response's validator is kept in `<id>.validator` and replayed as `If-Range`, so a
//! file that changed on the server is restarted from zero instead of being spliced together.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ureq::Timeout;

use crate::error::{Error, Result};
use crate::http::header;
use crate::model::Episode;

const CHUNK_SIZE: usize = 64 * 1024;
const MAX_FILE_STEM_CHARS: usize = 100;
/// Device names Windows refuses as file names, with or without an extension.
const RESERVED_STEMS: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

pub(crate) fn feed_dir(root: &Path, feed_title: &str, feed_id: i64) -> PathBuf {
    root.join(sanitize(feed_title, &format!("feed-{feed_id}")))
}

fn part_path(feed_dir: &Path, episode_id: i64) -> PathBuf {
    feed_dir.join(format!("{episode_id}.part"))
}

fn validator_path(feed_dir: &Path, episode_id: i64) -> PathBuf {
    feed_dir.join(format!("{episode_id}.validator"))
}

/// Removes the partial file and its validator, which only make sense together.
pub(crate) fn discard_partial(feed_dir: &Path, episode_id: i64) -> Result<()> {
    remove_if_present(&part_path(feed_dir, episode_id))?;
    remove_if_present(&validator_path(feed_dir, episode_id))
}

/// What a partial file needs in order to be continued: its length and the validator that the
/// server must still match.
struct Resume {
    offset: u64,
    validator: String,
}

pub(crate) fn fetch(
    agent: &ureq::Agent,
    root: &Path,
    feed_title: &str,
    episode: &Episode,
    progress: &mut dyn FnMut(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    let dir = feed_dir(root, feed_title, episode.feed_id);
    fs::create_dir_all(&dir)?;

    if let Some(resume) = resume_point(&dir, episode.id)? {
        check_cancelled(cancel)?;
        let response = request(agent, episode, Some(&resume))?;
        let status = response.status().as_u16();
        let range = header(&response, "content-range")
            .as_deref()
            .and_then(parse_content_range);
        match (status, range) {
            (206, Some(range)) if range.start == Some(resume.offset) => {
                let remaining = response.body().content_length();
                let total = range.total.or(remaining.map(|rest| rest + resume.offset));
                return save(
                    response,
                    &dir,
                    episode,
                    resume.offset,
                    total,
                    progress,
                    cancel,
                );
            }
            // The server ignored `Range` or declined `If-Range` because the file changed.
            (200, _) => return restart(response, &dir, episode, progress, cancel),
            // The partial file already holds everything the server has.
            (416, Some(range)) if range.total == Some(resume.offset) => {
                return finish(&dir, episode);
            }
            (206 | 416, _) => {}
            _ => return Err(status_error(status, episode)),
        }
        discard_partial(&dir, episode.id)?;
    }

    check_cancelled(cancel)?;
    let response = request(agent, episode, None)?;
    match response.status().as_u16() {
        200 => restart(response, &dir, episode, progress, cancel),
        status => Err(status_error(status, episode)),
    }
}

fn request(
    agent: &ureq::Agent,
    episode: &Episode,
    resume: Option<&Resume>,
) -> Result<ureq::http::Response<ureq::Body>> {
    let mut request = agent
        .get(&episode.enclosure_url)
        // A compressed body would make Range offsets and Content-Length meaningless.
        .header("Accept-Encoding", "identity");
    if let Some(resume) = resume {
        request = request
            .header("Range", format!("bytes={}-", resume.offset))
            .header("If-Range", &resume.validator);
    }
    request.call().map_err(|err| match err {
        ureq::Error::Timeout(Timeout::RecvResponse | Timeout::RecvBody) => Error::Stalled,
        other => other.into(),
    })
}

/// Starts the file over from the first byte of a full-body response and remembers its validator.
fn restart(
    response: ureq::http::Response<ureq::Body>,
    dir: &Path,
    episode: &Episode,
    progress: &mut dyn FnMut(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    let validator_file = validator_path(dir, episode.id);
    let validator = resume_token(
        header(&response, "etag").as_deref(),
        header(&response, "last-modified").as_deref(),
    );
    match validator {
        Some(validator) => fs::write(&validator_file, validator)?,
        // Without a validator the partial file could never be resumed safely.
        None => remove_if_present(&validator_file)?,
    }
    let total = response.body().content_length();
    save(response, dir, episode, 0, total, progress, cancel)
}

fn save(
    mut response: ureq::http::Response<ureq::Body>,
    dir: &Path,
    episode: &Episode,
    offset: u64,
    total: Option<u64>,
    progress: &mut dyn FnMut(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    let part = part_path(dir, episode.id);
    let mut file = if offset > 0 {
        OpenOptions::new().append(true).open(&part)?
    } else {
        File::create(&part)?
    };
    copy_body(
        response.body_mut().as_reader(),
        &mut file,
        offset,
        total,
        progress,
        cancel,
    )?;
    drop(file);
    finish(dir, episode)
}

/// A partial file is only resumable when its validator survived alongside it.
fn resume_point(dir: &Path, episode_id: i64) -> Result<Option<Resume>> {
    let offset = match fs::metadata(part_path(dir, episode_id)) {
        Ok(metadata) => metadata.len(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => 0,
        Err(err) => return Err(err.into()),
    };
    let validator = match fs::read_to_string(validator_path(dir, episode_id)) {
        Ok(text) => text.trim().to_owned(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err.into()),
    };
    if offset > 0 && !validator.is_empty() {
        return Ok(Some(Resume { offset, validator }));
    }
    discard_partial(dir, episode_id)?;
    Ok(None)
}

/// The value for `If-Range`: a strong ETag, else the modification date. Weak ETags are not
/// allowed there because they do not guarantee identical bytes.
fn resume_token(etag: Option<&str>, last_modified: Option<&str>) -> Option<String> {
    etag.map(str::trim)
        .filter(|tag| !tag.is_empty() && !tag.starts_with("W/"))
        .or(last_modified.map(str::trim))
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

fn check_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn status_error(status: u16, episode: &Episode) -> Error {
    Error::Status {
        status,
        url: episode.enclosure_url.clone(),
    }
}

fn copy_body(
    mut body: impl Read,
    file: &mut File,
    mut downloaded: u64,
    total: Option<u64>,
    progress: &mut dyn FnMut(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<()> {
    let mut buffer = vec![0u8; CHUNK_SIZE];
    progress(downloaded, total);
    loop {
        // The partial file stays on disk when cancelled so the next call can resume it.
        check_cancelled(cancel)?;
        let read = body.read(&mut buffer).map_err(read_error)?;
        if read == 0 {
            return Ok(());
        }
        file.write_all(&buffer[..read])?;
        downloaded += read as u64;
        progress(downloaded, total);
    }
}

fn finish(dir: &Path, episode: &Episode) -> Result<PathBuf> {
    let stem = sanitize(&episode.title, &format!("episode-{}", episode.id));
    let extension = file_extension(&episode.enclosure_url, episode.enclosure_type.as_deref());
    let mut target = dir.join(format!("{stem}.{extension}"));
    if target.exists() {
        target = dir.join(format!("{stem} ({}).{extension}", episode.id));
    }
    fs::rename(part_path(dir, episode.id), &target)?;
    remove_if_present(&validator_path(dir, episode.id))?;
    Ok(target)
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err.into()),
        _ => Ok(()),
    }
}

/// A read that times out means the server went quiet, which callers see as a stall.
fn read_error(err: io::Error) -> Error {
    match ureq::Error::from(err) {
        ureq::Error::Timeout(_) => Error::Stalled,
        ureq::Error::Io(err) => Error::Io(err),
        other => Error::Http(other),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentRange {
    /// `None` for the unsatisfied form `bytes */total`.
    start: Option<u64>,
    total: Option<u64>,
}

fn parse_content_range(value: &str) -> Option<ContentRange> {
    let spec = value.trim().strip_prefix("bytes")?.trim();
    let (range, total) = spec.split_once('/')?;
    let start = match range.trim() {
        "*" => None,
        range => Some(range.split_once('-')?.0.trim().parse().ok()?),
    };
    Some(ContentRange {
        start,
        total: total.trim().parse().ok(),
    })
}

/// Turns arbitrary feed text into a name every supported filesystem accepts.
pub(crate) fn sanitize(name: &str, fallback: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Trailing dots and spaces are illegal on Windows; leading dots hide the file elsewhere.
    let trimmed: String = replaced
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .chars()
        .take(MAX_FILE_STEM_CHARS)
        .collect();
    let trimmed = trimmed.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        return fallback.to_owned();
    }
    let device = trimmed
        .split('.')
        .next()
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    if RESERVED_STEMS.contains(&device.as_str()) {
        format!("_{trimmed}")
    } else {
        trimmed.to_owned()
    }
}

/// Prefers the URL's own extension, which is what players and tag readers expect.
fn file_extension(url: &str, mime_type: Option<&str>) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let from_url = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension)
        .filter(|extension| {
            (1..=4).contains(&extension.len())
                && extension.bytes().all(|b| b.is_ascii_alphanumeric())
        });
    if let Some(extension) = from_url {
        return extension.to_ascii_lowercase();
    }
    let from_type = match mime_type
        .map(|mime| mime.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("audio/x-m4a" | "audio/m4a" | "audio/mp4") => "m4a",
        Some("audio/aac") => "aac",
        Some("audio/ogg" | "application/ogg") => "ogg",
        Some("audio/opus") => "opus",
        Some("audio/wav" | "audio/x-wav") => "wav",
        Some("audio/flac") => "flac",
        Some("video/mp4") => "mp4",
        Some("video/x-m4v") => "m4v",
        Some("video/quicktime") => "mov",
        _ => "mp3",
    };
    from_type.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_forms() {
        assert_eq!(
            parse_content_range("bytes 100-199/200"),
            Some(ContentRange {
                start: Some(100),
                total: Some(200)
            })
        );
        assert_eq!(
            parse_content_range("bytes */200"),
            Some(ContentRange {
                start: None,
                total: Some(200)
            })
        );
        assert_eq!(
            parse_content_range("bytes 5-9/*"),
            Some(ContentRange {
                start: Some(5),
                total: None
            })
        );
        assert_eq!(parse_content_range("items 1-2/3"), None);
        assert_eq!(parse_content_range("bytes nonsense"), None);
    }

    #[test]
    fn resume_token_prefers_a_strong_etag_then_the_modification_date() {
        let date = Some("Mon, 20 Jan 2025 09:00:00 GMT");
        assert_eq!(
            resume_token(Some("\"abc\""), date).as_deref(),
            Some("\"abc\"")
        );
        assert_eq!(resume_token(Some("W/\"abc\""), date).as_deref(), date);
        assert_eq!(resume_token(None, date).as_deref(), date);
        assert_eq!(resume_token(Some("W/\"abc\""), None), None);
        assert_eq!(resume_token(Some("  "), Some(" ")), None);
        assert_eq!(resume_token(None, None), None);
    }

    #[test]
    fn sanitize_replaces_illegal_characters_and_trims() {
        assert_eq!(sanitize("What? A/B: \"Test\"", "x"), "What_ A_B_ _Test_");
        assert_eq!(sanitize("  ..Title.. ", "x"), "Title");
        assert_eq!(sanitize("???", "x"), "___");
        assert_eq!(sanitize("  ", "fallback"), "fallback");
    }

    #[test]
    fn sanitize_guards_windows_device_names_and_length() {
        assert_eq!(sanitize("CON", "x"), "_CON");
        assert_eq!(sanitize("nul.txt", "x"), "_nul.txt");
        assert_eq!(sanitize("console", "x"), "console");
        assert_eq!(
            sanitize(&"é".repeat(300), "x").chars().count(),
            MAX_FILE_STEM_CHARS
        );
    }

    #[test]
    fn extension_comes_from_the_url_then_the_mime_type() {
        assert_eq!(
            file_extension("https://cdn.example/a/Ep1.MP3?token=1.2", None),
            "mp3"
        );
        assert_eq!(
            file_extension("https://dts.example/redirect.mp3/host/file.m4a", None),
            "m4a"
        );
        assert_eq!(
            file_extension("https://example.com/stream/123", Some("audio/ogg")),
            "ogg"
        );
        assert_eq!(
            file_extension("https://example.com/stream/123", None),
            "mp3"
        );
        assert_eq!(
            file_extension("https://example.com/a.b/stream", Some("audio/mp4")),
            "m4a"
        );
    }
}
