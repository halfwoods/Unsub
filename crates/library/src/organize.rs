use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{Track, TrackQuery};
use crate::scan::FileFacts;

const MEDIA_FOLDER: &str = "media_folder";
const ORGANIZE: &str = "organize";
const COPY_ON_ADD: &str = "copy_on_add";
/// Most file systems cap a name at 255 bytes; this leaves room for " 12" and the extension.
const MAX_NAME_BYTES: usize = 200;
const MAX_COLLISIONS: u32 = 10_000;

/// iTunes' Advanced preferences: where the library keeps its files, and whether
/// it keeps them tidy and copies in what is added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSettings {
    pub folder: Option<PathBuf>,
    /// "Keep Media folder organized".
    pub organize: bool,
    /// "Copy files to Media folder when adding to library".
    pub copy_on_add: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizeProgress {
    pub done: u64,
    pub total: u64,
    pub current_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizeReport {
    pub moved: u64,
    pub unchanged: u64,
    /// Outside the media folder, still in the cloud, or missing: not ours to move.
    pub left_alone: u64,
    pub failed: Vec<(i64, String)>,
}

impl Library {
    /// Creates the folder if need be. Files already in it stay where they are
    /// until they are organized.
    pub fn set_media_folder(&mut self, path: &Path) -> Result<()> {
        fs::create_dir_all(path)?;
        let canonical = fs::canonicalize(path)?;
        let text = canonical
            .to_str()
            .ok_or_else(|| Error::NonUtf8Path(canonical.clone()))?;
        self.put_setting(MEDIA_FOLDER, text)
    }

    pub fn set_organize(&mut self, organize: bool) -> Result<()> {
        self.put_setting(ORGANIZE, flag(organize))
    }

    pub fn set_copy_on_add(&mut self, copy_on_add: bool) -> Result<()> {
        self.put_setting(COPY_ON_ADD, flag(copy_on_add))
    }

    pub fn media_settings(&self) -> Result<MediaSettings> {
        Ok(MediaSettings {
            folder: self.setting(MEDIA_FOLDER)?.map(PathBuf::from),
            organize: self.setting(ORGANIZE)?.as_deref() == Some("1"),
            copy_on_add: self.setting(COPY_ON_ADD)?.as_deref() == Some("1"),
        })
    }

    /// Moves every file inside the media folder to where its tags say it
    /// belongs. Files elsewhere are never touched, whatever the settings, and a
    /// file that cannot be moved is reported and left in place.
    pub fn organize_now(
        &mut self,
        mut progress: impl FnMut(OrganizeProgress),
    ) -> Result<OrganizeReport> {
        let media = self.media_settings()?.folder.ok_or(Error::NoMediaFolder)?;
        let mut tracks = self.tracks(&TrackQuery {
            kind: None,
            ..TrackQuery::default()
        })?;
        tracks.sort_by_key(|track| track.id);

        let total = tracks.len() as u64;
        let mut report = OrganizeReport::default();
        for (index, track) in tracks.iter().enumerate() {
            let current = Path::new(&track.path);
            if track.in_cloud || track.missing || !current.starts_with(&media) {
                report.left_alone += 1;
            } else {
                match self.move_into_place(track, &media) {
                    Ok(Some(new_path)) => match self.record_move(track.id, current, &new_path) {
                        Ok(()) => report.moved += 1,
                        Err(err) => return Err(err),
                    },
                    Ok(None) => report.unchanged += 1,
                    Err(err) => report.failed.push((track.id, err.to_string())),
                }
            }
            progress(OrganizeProgress {
                done: index as u64 + 1,
                total,
                current_path: track.path.clone(),
            });
        }
        Ok(report)
    }

    /// Where the file moved to, or none if it already sits in the right place.
    /// The database is left for the caller, which knows what else is changing.
    pub(crate) fn move_into_place(&self, track: &Track, media: &Path) -> Result<Option<PathBuf>> {
        let current = Path::new(&track.path);
        let target = self.free_path(organized_path(media, track), Some(current))?;
        if target == current {
            return Ok(None);
        }
        move_file(current, &target)?;
        if let Some(parent) = current.parent() {
            prune_empty_dirs(media, parent);
        }
        Ok(Some(target))
    }

    /// Points the row at the new location, moving the file back if that fails
    /// so the library and the disk never disagree.
    fn record_move(&self, id: i64, old: &Path, new: &Path) -> Result<()> {
        let text = new
            .to_str()
            .ok_or_else(|| Error::NonUtf8Path(new.to_path_buf()))?;
        let updated = self.conn.execute(
            "UPDATE tracks SET path = ?2 WHERE id = ?1",
            params![id, text],
        );
        if let Err(err) = updated {
            let _ = move_file(new, old);
            return Err(err.into());
        }
        Ok(())
    }

    /// Applies "copy on add" and "keep organized" to a file being added: one
    /// outside the media folder is copied in, one already inside is tidied.
    pub(crate) fn place_new_file(&self, track: &mut Track, settings: &MediaSettings) -> Result<()> {
        let Some(media) = &settings.folder else {
            return Ok(());
        };
        // A placeholder cannot be copied without downloading it.
        if track.in_cloud {
            return Ok(());
        }
        let current = PathBuf::from(&track.path);
        if current.starts_with(media) {
            if settings.organize
                && let Some(moved) = self.move_into_place(track, media)?
            {
                track.path = utf8(&moved)?;
            }
        } else if settings.copy_on_add {
            let target = self.free_path(organized_path(media, track), None)?;
            copy_file(&current, &target)?;
            let copy = FileFacts::of(&target)?;
            track.path = copy.path;
            track.file_size = copy.file_size;
            track.date_modified = copy.date_modified;
        }
        Ok(())
    }

    /// `wanted`, or the first "name 2", "name 3"... that nothing else uses on
    /// disk or in the library. A track's own current path always counts as free.
    pub(crate) fn free_path(&self, wanted: PathBuf, current: Option<&Path>) -> Result<PathBuf> {
        let stem = wanted
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let extension = wanted.extension().map(|e| e.to_string_lossy().into_owned());
        let mut candidate = wanted.clone();
        for attempt in 2..MAX_COLLISIONS {
            if Some(candidate.as_path()) == current {
                return Ok(candidate);
            }
            if !candidate.exists() && !self.path_in_library(&candidate)? {
                return Ok(candidate);
            }
            let name = match &extension {
                Some(extension) => format!("{stem} {attempt}.{extension}"),
                None => format!("{stem} {attempt}"),
            };
            candidate = wanted.with_file_name(name);
        }
        Err(io::Error::other("too many files with the same name").into())
    }

    pub(crate) fn path_in_library(&self, path: &Path) -> Result<bool> {
        let Some(text) = path.to_str() else {
            return Ok(false);
        };
        Ok(self
            .conn
            .prepare_cached("SELECT 1 FROM tracks WHERE path = ?1")?
            .exists([text])?)
    }

    fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    fn put_setting(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

fn flag(on: bool) -> &'static str {
    if on { "1" } else { "0" }
}

fn utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::NonUtf8Path(path.to_path_buf()))
}

/// `<media>/<Album Artist>/<Album>/<disc>-<track> <Title>.<ext>`, with the
/// disc only on multi-disc albums, as iTunes names them. Compilations file
/// under "Compilations".
pub(crate) fn organized_path(media: &Path, track: &Track) -> PathBuf {
    let artist = if track.compilation {
        "Compilations"
    } else if !track.album_artist.is_empty() {
        &track.album_artist
    } else {
        &track.artist
    };
    let multi_disc =
        track.disc_number.is_some_and(|d| d > 1) || track.disc_count.is_some_and(|c| c > 1);
    let number = match (track.disc_number.filter(|_| multi_disc), track.track_number) {
        (Some(disc), Some(number)) => format!("{disc}-{number:02} "),
        (None, Some(number)) => format!("{number:02} "),
        _ => String::new(),
    };
    let title = sanitize(&track.title, "Untitled");
    let mut name = sanitize(&format!("{number}{title}"), "Untitled");
    if let Some(extension) = Path::new(&track.path).extension() {
        name.push('.');
        name.push_str(&extension.to_string_lossy());
    }
    media
        .join(sanitize(artist, "Unknown Artist"))
        .join(sanitize(&track.album, "Unknown Album"))
        .join(name)
}

/// One path component that every common file system accepts and that the
/// scanner will not skip: no separators or reserved characters, no leading dot
/// (which would hide the file), no trailing dot or space, and a bounded length.
pub(crate) fn sanitize(name: &str, fallback: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = replaced.trim().trim_end_matches(['.', ' ']);
    let mut clean = String::new();
    for (index, c) in trimmed.chars().enumerate() {
        if clean.len() + c.len_utf8() > MAX_NAME_BYTES {
            break;
        }
        clean.push(if index == 0 && c == '.' { '_' } else { c });
    }
    let clean = clean.trim_end().to_owned();
    if clean.is_empty() {
        fallback.to_owned()
    } else {
        clean
    }
}

/// A rename where possible; across file systems, a copy that is flushed to disk
/// before the original goes.
pub(crate) fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(from, to) {
        Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
            copy_file(from, to)?;
            fs::remove_file(from)
        }
        other => other,
    }
}

pub(crate) fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(from, to)?;
    File::options().write(true).open(to)?.sync_all()
}

/// Removes the folders a move emptied, up to but never including the media folder.
pub(crate) fn prune_empty_dirs(media: &Path, from: &Path) {
    let mut dir = from;
    while dir != media && dir.starts_with(media) && fs::remove_dir(dir).is_ok() {
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MediaKind;

    fn track(path: &str) -> Track {
        Track {
            id: 1,
            path: path.to_owned(),
            title: "Halo".into(),
            artist: "Beyonc\u{e9}".into(),
            album_artist: String::new(),
            album: "I Am".into(),
            genre: String::new(),
            composer: String::new(),
            year: None,
            track_number: Some(3),
            track_count: None,
            disc_number: None,
            disc_count: None,
            duration_ms: 0,
            bitrate_kbps: None,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            codec: "MP3".into(),
            file_size: 0,
            date_added: 0,
            date_modified: 0,
            play_count: 0,
            last_played: None,
            skip_count: 0,
            rating: 0,
            kind: MediaKind::Music,
            compilation: false,
            enabled: true,
            in_cloud: false,
            missing: false,
            loudness: None,
            remember_position: false,
            has_lyrics: false,
            sort_title: None,
            sort_artist: None,
            sort_album: None,
            sort_album_artist: None,
        }
    }

    fn organized(track: &Track) -> String {
        organized_path(Path::new("/media"), track)
            .strip_prefix("/media")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn files_are_named_artist_album_number_title() {
        let mut halo = track("/old/x.MP3");
        assert_eq!(organized(&halo), "Beyonc\u{e9}/I Am/03 Halo.MP3");

        halo.album_artist = "Various".into();
        assert_eq!(
            organized(&halo),
            "Various/I Am/03 Halo.MP3",
            "the album artist wins"
        );

        halo.compilation = true;
        assert_eq!(organized(&halo), "Compilations/I Am/03 Halo.MP3");

        halo.compilation = false;
        halo.disc_number = Some(2);
        halo.disc_count = Some(2);
        assert_eq!(
            organized(&halo),
            "Various/I Am/2-03 Halo.MP3",
            "multi-disc albums name the disc"
        );

        halo.disc_number = Some(1);
        halo.disc_count = Some(1);
        assert_eq!(
            organized(&halo),
            "Various/I Am/03 Halo.MP3",
            "a single disc does not"
        );

        halo.track_number = None;
        assert_eq!(organized(&halo), "Various/I Am/Halo.MP3");
    }

    #[test]
    fn missing_details_get_placeholders() {
        let mut bare = track("/old/x.flac");
        bare.title.clear();
        bare.artist.clear();
        bare.album.clear();
        bare.track_number = None;

        assert_eq!(
            organized(&bare),
            "Unknown Artist/Unknown Album/Untitled.flac"
        );
        bare.path = "/old/noextension".into();
        assert_eq!(organized(&bare), "Unknown Artist/Unknown Album/Untitled");
    }

    #[test]
    fn names_are_made_safe_for_every_file_system() {
        let cases = [
            ("AC/DC", "AC_DC"),
            ("What?: A \"Title\" <1>", "What__ A _Title_ _1_"),
            ("back\\slash|pipe*", "back_slash_pipe_"),
            (".hidden", "_hidden"),
            ("..", "Fallback"),
            ("trailing dots...", "trailing dots"),
            ("  padded  ", "padded"),
            ("tab\there\u{7}", "tab_here_"),
            ("", "Fallback"),
            ("   ", "Fallback"),
            (
                "Beyonc\u{e9} \u{65e5}\u{672c}\u{8a9e}",
                "Beyonc\u{e9} \u{65e5}\u{672c}\u{8a9e}",
            ),
        ];
        for (name, expected) in cases {
            assert_eq!(sanitize(name, "Fallback"), expected, "{name:?}");
        }
    }

    #[test]
    fn long_names_are_cut_on_a_character_boundary() {
        let long = "\u{e9}".repeat(300);

        let clean = sanitize(&long, "x");

        assert!(clean.len() <= MAX_NAME_BYTES);
        assert!(clean.chars().all(|c| c == '\u{e9}'));
        assert!(clean.len() >= MAX_NAME_BYTES - 1);
    }

    #[test]
    fn pruning_stops_at_the_media_folder() {
        let dir = tempfile::TempDir::new().unwrap();
        let media = dir.path().join("media");
        let deep = media.join("Artist/Album");
        fs::create_dir_all(&deep).unwrap();
        fs::create_dir_all(media.join("Other")).unwrap();
        fs::write(media.join("Other/keep.txt"), b"x").unwrap();

        prune_empty_dirs(&media, &deep);

        assert!(!media.join("Artist").exists());
        assert!(media.exists(), "the media folder itself stays");
        prune_empty_dirs(&media, &media.join("Other"));
        assert!(
            media.join("Other").exists(),
            "a folder with something in it stays"
        );
        let outside = dir.path().join("elsewhere/empty");
        fs::create_dir_all(&outside).unwrap();
        prune_empty_dirs(&media, &outside);
        assert!(
            outside.exists(),
            "folders outside the media folder are never touched"
        );
    }
}
