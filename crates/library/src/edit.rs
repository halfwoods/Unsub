use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::tag::items::Timestamp;
use lofty::tag::{Accessor, ItemKey, Tag};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{MediaKind, Track, TrackOverrides};
use crate::scan::{FileFacts, now_unix, read_track};
use crate::store::{WriteMode, write_tracks};
use crate::tags;

/// What Get Info changes. Every field is optional and only those present are
/// applied, which is what makes a multi-track edit touch just the boxes that
/// were filled in. Text fields clear when empty; the numeric ones and the
/// artwork can be cleared explicitly with `Some(None)` (`null` in JSON).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackEdits {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    #[serde(deserialize_with = "set_or_clear")]
    pub year: Option<Option<i32>>,
    #[serde(deserialize_with = "set_or_clear")]
    pub track_number: Option<Option<u32>>,
    #[serde(deserialize_with = "set_or_clear")]
    pub track_count: Option<Option<u32>>,
    #[serde(deserialize_with = "set_or_clear")]
    pub disc_number: Option<Option<u32>>,
    #[serde(deserialize_with = "set_or_clear")]
    pub disc_count: Option<Option<u32>>,
    pub compilation: Option<bool>,
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
    /// 0..=100 in steps of 20. Kept in the library, not written to files.
    pub rating: Option<u8>,
    /// The song checkbox. Kept in the library, not written to files.
    pub enabled: Option<bool>,
    /// Get Info › Options › Remember playback position. Turning it off also
    /// forgets the place the track was left at.
    pub remember_position: Option<bool>,
    /// Kept in the library, and pinned so a rescan does not guess it afresh.
    pub kind: Option<MediaKind>,
    /// New cover art (any common image format), or `Some(None)` to remove it.
    #[serde(deserialize_with = "set_or_clear")]
    pub artwork: Option<Option<Vec<u8>>>,
}

/// Tells an absent field (leave alone) from an explicit `null` (clear it).
fn set_or_clear<'de, D, T>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditReport {
    /// Tracks whose files now carry the new tags.
    pub written: Vec<i64>,
    /// Tracks whose tags could not be written (read-only, an unsupported
    /// format, still in the cloud...): only the library was updated, and why.
    pub db_only: Vec<(i64, String)>,
    /// Tracks moved to follow their new details, with "keep organized" on.
    pub moved: Vec<i64>,
    pub move_failed: Vec<(i64, String)>,
}

impl TrackEdits {
    /// Anything a file's tags can hold.
    fn touches_tags(&self) -> bool {
        self.title.is_some()
            || self.artist.is_some()
            || self.album_artist.is_some()
            || self.album.is_some()
            || self.genre.is_some()
            || self.composer.is_some()
            || self.year.is_some()
            || self.track_number.is_some()
            || self.track_count.is_some()
            || self.disc_number.is_some()
            || self.disc_count.is_some()
            || self.compilation.is_some()
            || self.sort_title.is_some()
            || self.sort_artist.is_some()
            || self.sort_album.is_some()
            || self.sort_album_artist.is_some()
            || self.artwork.is_some()
    }

    /// What decides where "keep organized" files a track.
    fn affects_path(&self) -> bool {
        self.title.is_some()
            || self.artist.is_some()
            || self.album_artist.is_some()
            || self.album.is_some()
            || self.track_number.is_some()
            || self.disc_number.is_some()
            || self.disc_count.is_some()
            || self.compilation.is_some()
    }

    fn trimmed(&self) -> Self {
        let text = |value: &Option<String>| value.as_ref().map(|v| v.trim().to_owned());
        Self {
            title: text(&self.title),
            artist: text(&self.artist),
            album_artist: text(&self.album_artist),
            album: text(&self.album),
            genre: text(&self.genre),
            composer: text(&self.composer),
            sort_title: text(&self.sort_title),
            sort_artist: text(&self.sort_artist),
            sort_album: text(&self.sort_album),
            sort_album_artist: text(&self.sort_album_artist),
            ..self.clone()
        }
    }

    /// The library's side of the edit, for tracks whose files cannot carry it.
    fn apply_to(&self, track: &mut Track) {
        let text = |target: &mut String, value: &Option<String>| {
            if let Some(value) = value {
                target.clone_from(value);
            }
        };
        let sort = |target: &mut Option<String>, value: &Option<String>| {
            if let Some(value) = value {
                *target = Some(value.clone()).filter(|v| !v.is_empty());
            }
        };
        text(&mut track.title, &self.title);
        text(&mut track.artist, &self.artist);
        text(&mut track.album_artist, &self.album_artist);
        text(&mut track.album, &self.album);
        text(&mut track.genre, &self.genre);
        text(&mut track.composer, &self.composer);
        sort(&mut track.sort_title, &self.sort_title);
        sort(&mut track.sort_artist, &self.sort_artist);
        sort(&mut track.sort_album, &self.sort_album);
        sort(&mut track.sort_album_artist, &self.sort_album_artist);
        if let Some(year) = self.year {
            track.year = year;
        }
        if let Some(number) = self.track_number {
            track.track_number = number;
        }
        if let Some(count) = self.track_count {
            track.track_count = count;
        }
        if let Some(number) = self.disc_number {
            track.disc_number = number;
        }
        if let Some(count) = self.disc_count {
            track.disc_count = count;
        }
        if let Some(compilation) = self.compilation {
            track.compilation = compilation;
        }
        if let Some(kind) = self.kind {
            track.kind = kind;
        }
    }

    /// Keeps the pinned values in step with the edit, so a re-read cannot
    /// undo it: a field the file now carries is released, one it could not
    /// take is pinned.
    fn update_overrides(&self, overrides: &mut TrackOverrides, in_file: bool) {
        fn field<T: Clone>(slot: &mut Option<T>, edited: Option<T>, in_file: bool) {
            if let Some(value) = edited {
                *slot = if in_file { None } else { Some(value) };
            }
        }
        field(&mut overrides.title, self.title.clone(), in_file);
        field(&mut overrides.artist, self.artist.clone(), in_file);
        field(&mut overrides.album, self.album.clone(), in_file);
        field(
            &mut overrides.album_artist,
            self.album_artist.clone(),
            in_file,
        );
        field(&mut overrides.genre, self.genre.clone(), in_file);
        if let Some(year) = self.year {
            overrides.year = year.filter(|_| !in_file);
        }
        // The kind is not in the tags at all, so it is always pinned.
        if let Some(kind) = self.kind {
            overrides.kind = Some(kind);
        }
    }
}

struct Pending {
    track: Track,
    overrides: TrackOverrides,
    new_path: Option<PathBuf>,
    /// The file on disk was rewritten, so its size and time have changed.
    rewritten: bool,
}

impl Library {
    /// Get Info. For each track the tags are written into the file (every other
    /// tag and the audio untouched, replaced atomically) and the library row is
    /// updated to match. A file that cannot take the tags is reported in
    /// `db_only` and only its row changes; such edits are pinned so rescans
    /// keep them. With "keep organized" on, an edited track moves to its new place.
    ///
    /// Rating, the checkbox and kind live only in the library. A number
    /// (`year`, track and disc numbers) cleared on a file that cannot be
    /// written may come back when the file is next re-read.
    pub fn edit_tracks(&mut self, ids: &[i64], edits: &TrackEdits) -> Result<EditReport> {
        let edits = edits.trimmed();
        if let Some(rating) = edits.rating
            && (rating > 100 || !rating.is_multiple_of(20))
        {
            return Err(Error::InvalidRating(rating));
        }
        let picture = edits
            .artwork
            .as_ref()
            .map(|art| art.as_deref().map(picture_from).transpose())
            .transpose()?;

        let mut seen = HashSet::new();
        let mut tracks = Vec::new();
        for id in ids.iter().filter(|id| seen.insert(**id)) {
            tracks.push(self.track(*id)?.ok_or(Error::TrackNotFound(*id))?);
        }

        let media = self.media_settings()?;
        let now = now_unix();
        let mut report = EditReport::default();
        let mut pending = Vec::new();
        for mut track in tracks {
            let mut overrides = self.stored_overrides(track.id)?.unwrap_or_default();
            let mut in_file = false;
            if edits.touches_tags() {
                match unwritable(&track) {
                    Some(reason) => report.db_only.push((track.id, reason)),
                    None => match write_tags(Path::new(&track.path), &edits, picture.as_ref()) {
                        Ok(()) => {
                            in_file = true;
                            report.written.push(track.id);
                        }
                        Err(err) => report.db_only.push((track.id, err.to_string())),
                    },
                }
            }
            edits.update_overrides(&mut overrides, in_file);
            let pinned = Some(&overrides).filter(|o| **o != TrackOverrides::default());
            if in_file {
                let path = PathBuf::from(&track.path);
                let (id, rating, enabled) = (track.id, track.rating, track.enabled);
                track = read_track(&path, FileFacts::of(&path)?, now, pinned)?;
                (track.id, track.rating, track.enabled) = (id, rating, enabled);
            } else {
                edits.apply_to(&mut track);
            }

            let mut new_path = None;
            if edits.affects_path()
                && media.organize
                && let Some(folder) = &media.folder
                && Path::new(&track.path).starts_with(folder)
                && !track.in_cloud
                && !track.missing
            {
                match self.move_into_place(&track, folder) {
                    Ok(Some(moved)) => {
                        report.moved.push(track.id);
                        new_path = Some(moved);
                    }
                    Ok(None) => {}
                    Err(err) => report.move_failed.push((track.id, err.to_string())),
                }
            }
            pending.push(Pending {
                track,
                overrides,
                new_path,
                rewritten: in_file,
            });
        }

        let tx = self.conn.transaction()?;
        for Pending {
            mut track,
            overrides,
            new_path,
            rewritten,
        } in pending
        {
            if let Some(new_path) = &new_path {
                let text = new_path
                    .to_str()
                    .ok_or_else(|| Error::NonUtf8Path(new_path.clone()))?;
                tx.execute(
                    "UPDATE tracks SET path = ?2 WHERE id = ?1",
                    params![track.id, text],
                )?;
                track.path = text.to_owned();
            }
            if rewritten {
                // Only the tags changed, so a loudness measurement of the audio
                // still holds. The store keeps one while a file's size and time
                // are unchanged, so tell it the new ones first.
                tx.execute(
                    "UPDATE tracks SET file_size = ?2, date_modified = ?3 WHERE id = ?1",
                    params![track.id, track.file_size, track.date_modified],
                )?;
            }
            if edits.touches_tags() || edits.kind.is_some() {
                write_tracks(&tx, WriteMode::RefreshTags, [&track])?;
            }
            let json = (overrides != TrackOverrides::default())
                .then(|| serde_json::to_string(&overrides))
                .transpose()?;
            tx.execute(
                "UPDATE tracks SET overrides = ?2, rating = COALESCE(?3, rating),
                                   enabled = COALESCE(?4, enabled),
                                   remember_position = COALESCE(?5, remember_position),
                                   bookmark_ms = CASE WHEN ?5 = 0 THEN NULL ELSE bookmark_ms END
                 WHERE id = ?1",
                params![
                    track.id,
                    json,
                    edits.rating,
                    edits.enabled,
                    edits.remember_position
                ],
            )?;
        }
        tx.commit()?;
        Ok(report)
    }

    pub(crate) fn stored_overrides(&self, id: i64) -> Result<Option<TrackOverrides>> {
        let json: Option<Option<String>> = self
            .conn
            .query_row("SELECT overrides FROM tracks WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(json
            .flatten()
            .map(|json| serde_json::from_str(&json))
            .transpose()?)
    }
}

/// Why the file cannot be written, if it plainly cannot.
fn unwritable(track: &Track) -> Option<String> {
    if track.in_cloud {
        return Some("the file is in iCloud and has not been downloaded".to_owned());
    }
    match fs::metadata(&track.path) {
        Err(_) => Some("the file is missing".to_owned()),
        Ok(metadata) if metadata.permissions().readonly() => {
            Some("the file is read-only".to_owned())
        }
        Ok(_) => None,
    }
}

fn picture_from(bytes: &[u8]) -> Result<Picture> {
    let mut picture = Picture::from_reader(&mut Cursor::new(bytes))
        .map_err(|err| Error::InvalidArtwork(err.to_string()))?;
    picture.set_pic_type(PictureType::CoverFront);
    Ok(picture)
}

fn write_tags(path: &Path, edits: &TrackEdits, picture: Option<&Option<Picture>>) -> Result<()> {
    replace_atomically(path, |temp| {
        // Cover art is read too (the default), or saving would drop what is there.
        // The copy has a made-up extension, so the format is sniffed from content.
        let mut tagged = tags::probe(temp, ParseOptions::new())?.read()?;
        if tagged.primary_tag().is_none() {
            tagged.insert_tag(Tag::new(tagged.primary_tag_type()));
        }
        let Some(tag) = tagged.primary_tag_mut() else {
            return Err(Error::UnsupportedFormat);
        };
        apply_text(tag, edits);
        if let Some(picture) = picture {
            replace_pictures(&mut tagged, picture.as_ref());
        }
        tagged.save_to_path(temp, WriteOptions::default())?;
        Ok(())
    })
}

fn apply_text(tag: &mut Tag, edits: &TrackEdits) {
    let text = [
        (ItemKey::TrackTitle, &edits.title),
        (ItemKey::TrackArtist, &edits.artist),
        (ItemKey::AlbumArtist, &edits.album_artist),
        (ItemKey::AlbumTitle, &edits.album),
        (ItemKey::Genre, &edits.genre),
        (ItemKey::Composer, &edits.composer),
        (ItemKey::TrackTitleSortOrder, &edits.sort_title),
        (ItemKey::TrackArtistSortOrder, &edits.sort_artist),
        (ItemKey::AlbumTitleSortOrder, &edits.sort_album),
        (ItemKey::AlbumArtistSortOrder, &edits.sort_album_artist),
    ];
    for (key, value) in text {
        if let Some(value) = value {
            set(tag, key, Some(value.as_str()).filter(|v| !v.is_empty()));
        }
    }
    let numbers = [
        (ItemKey::TrackNumber, edits.track_number),
        (ItemKey::TrackTotal, edits.track_count),
        (ItemKey::DiscNumber, edits.disc_number),
        (ItemKey::DiscTotal, edits.disc_count),
    ];
    for (key, value) in numbers {
        if let Some(value) = value {
            set(tag, key, value.map(|n| n.to_string()).as_deref());
        }
    }
    if let Some(year) = edits.year {
        match year.and_then(|y| u16::try_from(y).ok()) {
            Some(year) => tag.set_date(Timestamp {
                year,
                ..Timestamp::default()
            }),
            None => tag.remove_date(),
        }
    }
    if let Some(compilation) = edits.compilation {
        set(tag, ItemKey::FlagCompilation, compilation.then_some("1"));
    }
}

fn set(tag: &mut Tag, key: ItemKey, value: Option<&str>) {
    tag.remove_key(key);
    if let Some(value) = value {
        tag.insert_text(key, value.to_owned());
    }
}

/// Art can sit in more than one of a file's tags, and readers take the first
/// they find, so the old pictures go from all of them before the new one is
/// placed in the main tag.
fn replace_pictures(tagged: &mut lofty::file::TaggedFile, new: Option<&Picture>) {
    let types: Vec<_> = tagged.tags().iter().map(Tag::tag_type).collect();
    for tag_type in types {
        if let Some(tag) = tagged.tag_mut(tag_type) {
            while tag.picture_count() > 0 {
                tag.remove_picture(0);
            }
        }
    }
    if let (Some(picture), Some(tag)) = (new, tagged.primary_tag_mut()) {
        tag.push_picture(picture.clone());
    }
}

/// Edits a copy beside the original and renames it into place, so a crash or
/// a failed write leaves the original as it was. The copy is hidden while it
/// exists, which keeps a concurrent scan from adding it.
pub(crate) fn replace_atomically(
    path: &Path,
    fill: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = fs::copy(path, &temp)
        .map(|_| ())
        .map_err(Error::from)
        .and_then(|()| fill(&temp))
        .and_then(|()| Ok(File::options().write(true).open(&temp)?.sync_all()?))
        .and_then(|()| Ok(fs::rename(&temp, path)?));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contents(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_failed_write_leaves_the_original_untouched_and_no_temporary_behind() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("song.mp3");
        fs::write(&path, b"original audio").unwrap();

        let result = replace_atomically(&path, |temp| {
            // Half-way through: the copy has been damaged, then the write fails.
            fs::write(temp, b"half written")?;
            Err(Error::UnsupportedFormat)
        });

        assert!(matches!(result, Err(Error::UnsupportedFormat)));
        assert_eq!(fs::read(&path).unwrap(), b"original audio");
        assert_eq!(contents(dir.path()), ["song.mp3"]);
    }

    #[test]
    fn a_successful_write_replaces_the_file_in_place() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("song.mp3");
        fs::write(&path, b"original audio").unwrap();

        replace_atomically(&path, |temp| {
            assert!(
                temp.file_name().unwrap().to_string_lossy().starts_with('.'),
                "hidden while it exists"
            );
            assert_eq!(
                temp.parent(),
                path.parent(),
                "same directory, so the rename is atomic"
            );
            fs::write(temp, b"new bytes")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new bytes");
        assert_eq!(contents(dir.path()), ["song.mp3"]);
    }

    #[test]
    fn a_missing_original_fails_without_creating_anything() {
        let dir = tempfile::TempDir::new().unwrap();

        let result = replace_atomically(&dir.path().join("gone.mp3"), |_| Ok(()));

        assert!(matches!(result, Err(Error::Io(_))));
        assert!(contents(dir.path()).is_empty());
    }

    #[test]
    fn a_rename_that_fails_cleans_up_too() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("song.mp3");
        fs::write(&path, b"original").unwrap();

        // Replacing the file with a directory of the same name makes the rename fail.
        let result = replace_atomically(&path, |_| {
            fs::remove_file(&path)?;
            fs::create_dir(&path)?;
            Ok(())
        });

        assert!(result.is_err());
        assert!(path.is_dir());
        assert_eq!(
            contents(dir.path()),
            ["song.mp3"],
            "the hidden copy is gone"
        );
    }

    #[test]
    fn absent_fields_stay_absent_and_null_clears() {
        let edits: TrackEdits = serde_json::from_str(
            r#"{"title":"T","year":null,"trackNumber":3,"artwork":null,"rating":40}"#,
        )
        .unwrap();

        assert_eq!(edits.title.as_deref(), Some("T"));
        assert_eq!(edits.year, Some(None), "null means clear");
        assert_eq!(edits.track_number, Some(Some(3)));
        assert_eq!(edits.disc_number, None, "absent means leave alone");
        assert_eq!(edits.artwork, Some(None));
        assert_eq!(edits.rating, Some(40));
        assert_eq!(
            serde_json::from_str::<TrackEdits>("{}").unwrap(),
            TrackEdits::default()
        );
    }
}
