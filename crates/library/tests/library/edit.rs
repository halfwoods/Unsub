use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::time::{Duration, SystemTime};

use image::{ImageFormat, Rgb, RgbImage};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::id3::v2::{BinaryFrame, Frame, FrameId};
use lofty::mpeg::MpegFile;
use lofty::tag::{Accessor, ItemKey, Tag};
use tempfile::TempDir;
use unsub_library::{Error, Library, MediaKind, TrackEdits, TrackOverrides, TrackQuery};

use crate::common::{
    added_ids, all_kinds, copy_fixtures, fixtures_dir, library_of, scan, scanned_library, track,
    track_titled,
};

fn file_tag(path: &Path) -> Tag {
    lofty::read_from_path(path)
        .unwrap()
        .primary_tag()
        .unwrap()
        .clone()
}

fn duration(path: &Path) -> Duration {
    lofty::read_from_path(path).unwrap().properties().duration()
}

/// The encoded audio of a file, without its tags, for formats where that is
/// easy to find. Two files with equal payloads carry the same sound.
fn audio_payload(path: &Path) -> Option<Vec<u8>> {
    let bytes = fs::read(path).unwrap();
    match path.extension()?.to_str()? {
        "mp3" => {
            let start = if bytes.starts_with(b"ID3") {
                let size = bytes[6..10]
                    .iter()
                    .fold(0usize, |acc, b| (acc << 7) | usize::from(*b & 0x7f));
                10 + size
            } else {
                0
            };
            Some(bytes[start..].to_vec())
        }
        "flac" => {
            let mut position = 4;
            loop {
                let header = bytes[position];
                let length = usize::from(bytes[position + 1]) << 16
                    | usize::from(bytes[position + 2]) << 8
                    | usize::from(bytes[position + 3]);
                position += 4 + length;
                if header & 0x80 != 0 {
                    return Some(bytes[position..].to_vec());
                }
            }
        }
        "m4a" | "m4b" => {
            let mut position = 0;
            while position + 8 <= bytes.len() {
                let size =
                    u32::from_be_bytes(bytes[position..position + 4].try_into().unwrap()) as usize;
                let size = if size == 0 {
                    bytes.len() - position
                } else {
                    size
                };
                if &bytes[position + 4..position + 8] == b"mdat" {
                    return Some(bytes[position + 8..position + size].to_vec());
                }
                position += size;
            }
            None
        }
        "wav" => {
            let mut position = 12;
            while position + 8 <= bytes.len() {
                let size = u32::from_le_bytes(bytes[position + 4..position + 8].try_into().unwrap())
                    as usize;
                if &bytes[position..position + 4] == b"data" {
                    return Some(bytes[position + 8..position + 8 + size].to_vec());
                }
                position += 8 + size + (size & 1);
            }
            None
        }
        _ => None,
    }
}

fn picture(format: ImageFormat, color: [u8; 3]) -> Vec<u8> {
    let mut bytes = Vec::new();
    RgbImage::from_pixel(24, 24, Rgb(color))
        .write_to(&mut Cursor::new(&mut bytes), format)
        .unwrap();
    bytes
}

fn full_edit() -> TrackEdits {
    TrackEdits {
        title: Some("New Title".into()),
        artist: Some("New Artist".into()),
        album_artist: Some("New Album Artist".into()),
        album: Some("New Album".into()),
        genre: Some("New Genre".into()),
        composer: Some("New Composer".into()),
        year: Some(Some(2001)),
        track_number: Some(Some(7)),
        track_count: Some(Some(12)),
        disc_number: Some(Some(2)),
        disc_count: Some(Some(3)),
        compilation: Some(true),
        sort_title: Some("Sort Title".into()),
        sort_artist: Some("Sort Artist".into()),
        sort_album: Some("Sort Album".into()),
        sort_album_artist: Some("Sort Album Artist".into()),
        ..TrackEdits::default()
    }
}

fn dir_listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn a_full_edit_reaches_the_file_and_the_library_for_every_rich_format() {
    let (mut library, dir) = scanned_library();
    for (file, title) in [
        ("come_together.flac", "Come Together"),
        ("something.m4a", "Something"),
        ("halo.mp3", "Halo"),
        ("zombie.ogg", "Zombie"),
        ("chapter_1.m4b", "Chapter 1"),
    ] {
        let id = track_titled(&library, title).id;
        let path = dir.path().join(file);
        let (before_audio, before_length) = (audio_payload(&path), duration(&path));

        let report = library.edit_tracks(&[id], &full_edit()).unwrap();

        assert_eq!(report.written, [id], "{file}");
        assert!(report.db_only.is_empty(), "{file}: {:?}", report.db_only);
        let tag = file_tag(&path);
        assert_eq!(tag.title().as_deref(), Some("New Title"), "{file}");
        assert_eq!(tag.artist().as_deref(), Some("New Artist"), "{file}");
        assert_eq!(tag.album().as_deref(), Some("New Album"), "{file}");
        assert_eq!(tag.genre().as_deref(), Some("New Genre"), "{file}");
        assert_eq!(
            tag.get_string(ItemKey::AlbumArtist),
            Some("New Album Artist"),
            "{file}"
        );
        assert_eq!(
            tag.get_string(ItemKey::Composer),
            Some("New Composer"),
            "{file}"
        );
        assert_eq!(tag.date().map(|d| d.year), Some(2001), "{file}");
        assert_eq!(
            (tag.track(), tag.track_total()),
            (Some(7), Some(12)),
            "{file}"
        );
        assert_eq!((tag.disk(), tag.disk_total()), (Some(2), Some(3)), "{file}");
        assert_eq!(
            tag.get_string(ItemKey::FlagCompilation),
            Some("1"),
            "{file}"
        );
        assert_eq!(
            tag.get_string(ItemKey::TrackTitleSortOrder),
            Some("Sort Title"),
            "{file}"
        );
        assert_eq!(
            tag.get_string(ItemKey::TrackArtistSortOrder),
            Some("Sort Artist"),
            "{file}"
        );
        assert_eq!(
            tag.get_string(ItemKey::AlbumTitleSortOrder),
            Some("Sort Album"),
            "{file}"
        );
        assert_eq!(
            tag.get_string(ItemKey::AlbumArtistSortOrder),
            Some("Sort Album Artist"),
            "{file}"
        );

        let row = library.track(id).unwrap().unwrap();
        assert_eq!(
            (row.title.as_str(), row.artist.as_str(), row.album.as_str()),
            ("New Title", "New Artist", "New Album")
        );
        assert_eq!(
            (
                row.album_artist.as_str(),
                row.genre.as_str(),
                row.composer.as_str()
            ),
            ("New Album Artist", "New Genre", "New Composer")
        );
        assert_eq!(
            (row.year, row.track_number, row.track_count),
            (Some(2001), Some(7), Some(12))
        );
        assert_eq!(
            (row.disc_number, row.disc_count, row.compilation),
            (Some(2), Some(3), true)
        );
        assert_eq!(row.sort_title.as_deref(), Some("Sort Title"));
        assert_eq!(row.sort_album_artist.as_deref(), Some("Sort Album Artist"));

        assert_eq!(duration(&path), before_length, "{file}: same audio length");
        if let Some(before) = before_audio {
            assert_eq!(
                audio_payload(&path).unwrap(),
                before,
                "{file}: audio bytes untouched"
            );
        }
        assert!(
            dir_listing(dir.path())
                .iter()
                .all(|name| !name.ends_with(".tmp")),
            "no temporary file left behind"
        );
    }
}

#[test]
fn a_file_with_few_tag_fields_takes_what_it_can() {
    let (mut library, dir) = scanned_library();
    let id = track_titled(&library, "untagged").id;
    let path = dir.path().join("untagged.wav");
    let audio = audio_payload(&path).unwrap();

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                title: Some("Tagged".into()),
                artist: Some("Someone".into()),
                album: Some("Somewhere".into()),
                genre: Some("Noise".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(report.written, [id]);
    let tag = file_tag(&path);
    assert_eq!(
        (tag.title().as_deref(), tag.artist().as_deref()),
        (Some("Tagged"), Some("Someone"))
    );
    assert_eq!(
        (tag.album().as_deref(), tag.genre().as_deref()),
        (Some("Somewhere"), Some("Noise"))
    );
    assert_eq!(audio_payload(&path).unwrap(), audio);
    let row = library.track(id).unwrap().unwrap();
    assert_eq!(
        (row.title.as_str(), row.artist.as_str(), row.album.as_str()),
        ("Tagged", "Someone", "Somewhere")
    );
}

#[test]
fn a_multi_track_edit_changes_only_the_fields_that_are_set() {
    let (mut library, dir) = scanned_library();
    let ids: Vec<i64> = ["Come Together", "Something", "Halo"]
        .iter()
        .map(|t| track_titled(&library, t).id)
        .collect();
    let before: Vec<_> = ids
        .iter()
        .map(|id| library.track(*id).unwrap().unwrap())
        .collect();

    let report = library
        .edit_tracks(
            &ids,
            &TrackEdits {
                genre: Some("Jazz".into()),
                year: Some(Some(1999)),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(report.written, ids);
    for (id, old) in ids.iter().zip(&before) {
        let row = library.track(*id).unwrap().unwrap();
        assert_eq!((row.genre.as_str(), row.year), ("Jazz", Some(1999)));
        assert_eq!(
            (
                row.title.as_str(),
                row.artist.as_str(),
                row.album.as_str(),
                row.track_number,
                row.disc_number
            ),
            (
                old.title.as_str(),
                old.artist.as_str(),
                old.album.as_str(),
                old.track_number,
                old.disc_number
            )
        );
        assert_eq!(
            (row.play_count, row.rating, row.date_added, row.duration_ms),
            (old.play_count, old.rating, old.date_added, old.duration_ms)
        );
        let tag = file_tag(Path::new(&row.path));
        assert_eq!(
            tag.title().as_deref(),
            Some(old.title.as_str()),
            "the file's title is untouched"
        );
        assert_eq!(tag.genre().as_deref(), Some("Jazz"));
    }
    drop(dir);
}

#[test]
fn fields_can_be_cleared() {
    let (mut library, dir) = scanned_library();
    let id = track_titled(&library, "Come Together").id;
    library.edit_tracks(&[id], &full_edit()).unwrap();

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                genre: Some(String::new()),
                composer: Some(String::new()),
                year: Some(None),
                track_number: Some(None),
                track_count: Some(None),
                disc_count: Some(None),
                compilation: Some(false),
                sort_artist: Some(String::new()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(report.written, [id]);
    let tag = file_tag(&dir.path().join("come_together.flac"));
    assert_eq!(tag.genre(), None);
    assert_eq!(tag.get_string(ItemKey::Composer), None);
    assert_eq!(tag.date(), None);
    assert_eq!(
        (tag.track(), tag.track_total(), tag.disk_total()),
        (None, None, None)
    );
    assert_eq!(tag.disk(), Some(2), "left alone");
    assert_eq!(tag.get_string(ItemKey::FlagCompilation), None);
    assert_eq!(tag.get_string(ItemKey::TrackArtistSortOrder), None);
    let row = library.track(id).unwrap().unwrap();
    assert_eq!(
        (row.genre.as_str(), row.composer.as_str(), row.year),
        ("", "", None)
    );
    assert_eq!(
        (row.track_number, row.track_count, row.disc_count),
        (None, None, None)
    );
    assert!(!row.compilation && row.sort_artist.is_none());
    assert_eq!(row.sort_title.as_deref(), Some("Sort Title"));
}

#[test]
fn artwork_can_be_replaced_set_and_cleared_without_touching_the_audio() {
    let (mut library, dir) = scanned_library();
    let flac = track_titled(&library, "Come Together").id;
    let mp3 = track_titled(&library, "Halo").id;
    let m4a = track_titled(&library, "Something").id;
    let green = picture(ImageFormat::Jpeg, [0, 200, 0]);
    let blue = picture(ImageFormat::Png, [0, 0, 200]);
    let audio = |name: &str| audio_payload(&dir.path().join(name)).unwrap();
    let (flac_audio, mp3_audio, m4a_audio) = (
        audio("come_together.flac"),
        audio("halo.mp3"),
        audio("something.m4a"),
    );
    assert_eq!(
        library.artwork(flac).unwrap().unwrap().mime,
        "image/png",
        "the fixture starts with a red PNG"
    );

    library
        .edit_tracks(
            &[flac],
            &TrackEdits {
                artwork: Some(Some(green.clone())),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    library
        .edit_tracks(
            &[mp3, m4a],
            &TrackEdits {
                artwork: Some(Some(blue.clone())),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    let replaced = library.artwork(flac).unwrap().unwrap();
    assert_eq!(
        (replaced.mime.as_str(), replaced.bytes),
        ("image/jpeg", green)
    );
    for id in [mp3, m4a] {
        let added = library.artwork(id).unwrap().unwrap();
        assert_eq!(
            (added.mime.as_str(), added.bytes),
            ("image/png", blue.clone())
        );
    }

    library
        .edit_tracks(
            &[flac, mp3],
            &TrackEdits {
                artwork: Some(None),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert!(library.artwork(flac).unwrap().is_none());
    assert!(library.artwork(mp3).unwrap().is_none());
    assert!(
        library.artwork(m4a).unwrap().is_some(),
        "untouched tracks keep theirs"
    );
    assert_eq!(audio("come_together.flac"), flac_audio);
    assert_eq!(audio("halo.mp3"), mp3_audio);
    assert_eq!(audio("something.m4a"), m4a_audio);
}

#[test]
fn text_edits_keep_the_existing_artwork_and_unknown_tag_items() {
    let (mut library, dir) = scanned_library();
    let flac = dir.path().join("come_together.flac");
    let mut tagged = lofty::read_from_path(&flac).unwrap();
    tagged
        .primary_tag_mut()
        .unwrap()
        .insert_text(ItemKey::ReplayGainTrackGain, "-6.5 dB".into());
    tagged.save_to_path(&flac, WriteOptions::default()).unwrap();
    let mp3 = dir.path().join("halo.mp3");
    let mut mpeg = MpegFile::read_from(
        &mut fs::File::open(&mp3).unwrap(),
        lofty::config::ParseOptions::new(),
    )
    .unwrap();
    mpeg.id3v2_mut()
        .unwrap()
        .insert(Frame::Binary(BinaryFrame::new(
            FrameId::new("PCST").unwrap(),
            vec![0; 4],
        )));
    mpeg.save_to_path(&mp3, WriteOptions::default()).unwrap();
    library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    let flac_id = track_titled(&library, "Come Together").id;
    let mp3_id = track_titled(&library, "Halo").id;
    assert_eq!(
        library.track(mp3_id).unwrap().unwrap().kind,
        MediaKind::Podcast
    );

    library
        .edit_tracks(
            &[flac_id, mp3_id],
            &TrackEdits {
                album: Some("Elsewhere".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(
        file_tag(&flac).get_string(ItemKey::ReplayGainTrackGain),
        Some("-6.5 dB")
    );
    assert!(
        library.artwork(flac_id).unwrap().is_some(),
        "the cover survives a text edit"
    );
    let mpeg = MpegFile::read_from(
        &mut fs::File::open(&mp3).unwrap(),
        lofty::config::ParseOptions::new(),
    )
    .unwrap();
    assert!(
        mpeg.id3v2()
            .unwrap()
            .get(&FrameId::new("PCST").unwrap())
            .is_some(),
        "a frame the library does not model is kept"
    );
    assert_eq!(
        library.track(mp3_id).unwrap().unwrap().kind,
        MediaKind::Podcast
    );
}

#[test]
fn files_that_cannot_be_written_are_updated_in_the_library_only() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());
    let cloud = dir.path().join("in the cloud.mp3");
    fs::write(&cloud, b"placeholder").unwrap();
    fs::write(cloud.with_extension("evicted"), b"").unwrap();
    scan(&mut library, dir.path());
    let id = |title: &str| track_titled(&library, title).id;
    let (good, readonly, cloud_id) = (id("Halo"), id("Zombie"), id("in the cloud"));
    // Rows whose files are missing, or are not audio at all, as a stale library might hold.
    let garbage = dir.path().join("garbage.mp3");
    fs::write(&garbage, b"not an mp3 at all").unwrap();
    let mut gone = track(900, "gone", "x", "y", 1);
    gone.path = dir.path().join("gone.mp3").to_str().unwrap().to_owned();
    let mut broken = track(901, "garbage", "x", "y", 1);
    broken.path = garbage.to_str().unwrap().to_owned();
    library.import_tracks(&[gone, broken]).unwrap();
    let garbage_id = 901;

    let readonly_path = dir.path().join("zombie.ogg");
    let mut permissions = fs::metadata(&readonly_path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&readonly_path, permissions).unwrap();
    let untouched = fs::read(&readonly_path).unwrap();
    let garbage_bytes = fs::read(&garbage).unwrap();

    let report = library
        .edit_tracks(
            &[good, readonly, garbage_id, cloud_id, 900],
            &TrackEdits {
                title: Some("Edited".into()),
                genre: Some("Edited Genre".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(report.written, [good]);
    let reasons: Vec<(i64, &str)> = report
        .db_only
        .iter()
        .map(|(id, why)| (*id, why.as_str()))
        .collect();
    assert_eq!(reasons.len(), 4, "{reasons:?}");
    let why = |id: i64| {
        reasons
            .iter()
            .find(|(i, _)| *i == id)
            .unwrap()
            .1
            .to_lowercase()
    };
    assert!(why(readonly).contains("read-only"));
    assert!(why(cloud_id).contains("icloud"));
    assert!(why(900).contains("missing"));
    assert!(!why(garbage_id).is_empty());
    for id in [good, readonly, garbage_id, cloud_id, 900] {
        let row = library.track(id).unwrap().unwrap();
        assert_eq!(
            (row.title.as_str(), row.genre.as_str()),
            ("Edited", "Edited Genre"),
            "{id}"
        );
    }
    assert_eq!(
        fs::read(&readonly_path).unwrap(),
        untouched,
        "a read-only file is not rewritten"
    );
    assert_eq!(fs::read(&garbage).unwrap(), garbage_bytes);
    assert!(
        dir_listing(dir.path())
            .iter()
            .all(|name| !name.ends_with(".tmp"))
    );
    let mut permissions = fs::metadata(&readonly_path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&readonly_path, permissions).unwrap();
}

#[test]
fn library_only_edits_are_pinned_so_a_rescan_keeps_them() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("Artist/Album/05 - Track.mp3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::copy(fixtures_dir().join("halo.mp3"), &path).unwrap();
    fs::write(path.with_extension("evicted"), b"").unwrap();
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());
    scan(&mut library, dir.path());
    let id = library.tracks(&all_kinds()).unwrap()[0].id;
    assert!(library.track(id).unwrap().unwrap().in_cloud);

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                title: Some("Pinned".into()),
                genre: Some("Pinned Genre".into()),
                year: Some(Some(1984)),
                composer: Some("Pinned Composer".into()),
                track_number: Some(Some(9)),
                track_count: Some(Some(10)),
                disc_number: Some(Some(2)),
                disc_count: Some(Some(3)),
                compilation: Some(true),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(report.db_only.len(), 1);
    // The download arrives and the file is read properly; the edit still stands.
    fs::remove_file(path.with_extension("evicted")).unwrap();
    scan(&mut library, dir.path());
    let row = library.track(id).unwrap().unwrap();
    assert!(!row.in_cloud);
    assert_eq!(
        (row.title.as_str(), row.genre.as_str(), row.year),
        ("Pinned", "Pinned Genre", Some(1984))
    );
    assert_eq!(
        (
            row.composer.as_str(),
            row.track_number,
            row.track_count,
            row.disc_number,
            row.disc_count,
            row.compilation
        ),
        ("Pinned Composer", Some(9), Some(10), Some(2), Some(3), true)
    );
    assert_eq!(row.codec, "MP3");
    assert!(row.duration_ms > 0, "everything else comes from the file");
}

#[test]
fn writing_a_field_releases_its_pinned_value() {
    let (_dir, path) = single_file();
    let mut library = Library::open_in_memory().unwrap();
    let overrides = TrackOverrides {
        title: Some("Pinned".into()),
        artist: Some("Pinned Artist".into()),
        ..TrackOverrides::default()
    };
    let id = added_ids(&mut library, &[(path.clone(), overrides)])[0];

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                title: Some("Fresh".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(report.written, [id]);
    let row = library.track(id).unwrap().unwrap();
    assert_eq!(
        (row.title.as_str(), row.artist.as_str()),
        ("Fresh", "Pinned Artist")
    );
    // A later re-read gives the new title, since the file now says so and the pin is gone.
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(900))
        .unwrap();
    library
        .add_paths(&[path.parent().unwrap().to_path_buf()], |_| {})
        .unwrap();
    let row = library.track(id).unwrap().unwrap();
    assert_eq!(
        (row.title.as_str(), row.artist.as_str()),
        ("Fresh", "Pinned Artist")
    );
}

fn single_file() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("halo.mp3");
    fs::copy(fixtures_dir().join("halo.mp3"), &path).unwrap();
    (dir, path)
}

#[test]
fn the_kind_is_pinned_across_rescans() {
    let (dir, path) = single_file();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let id = library.tracks(&all_kinds()).unwrap()[0].id;

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                kind: Some(MediaKind::Audiobook),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert!(
        report.written.is_empty() && report.db_only.is_empty(),
        "kind is not a tag"
    );
    assert!(library.tracks(&TrackQuery::default()).unwrap().is_empty());
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(900))
        .unwrap();
    library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!(
        library.track(id).unwrap().unwrap().kind,
        MediaKind::Audiobook
    );
}

#[test]
fn rating_and_the_checkbox_are_library_only() {
    let (mut library, dir) = scanned_library();
    let id = track_titled(&library, "Halo").id;
    let path = dir.path().join("halo.mp3");
    let bytes = fs::read(&path).unwrap();

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                rating: Some(80),
                enabled: Some(false),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert!(report.written.is_empty() && report.db_only.is_empty());
    let row = library.track(id).unwrap().unwrap();
    assert_eq!((row.rating, row.enabled), (80, false));
    assert_eq!(
        fs::read(&path).unwrap(),
        bytes,
        "the file was not rewritten"
    );
}

#[test]
fn bad_requests_change_nothing() {
    let (mut library, dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    let before = library.track(halo).unwrap().unwrap();
    let bytes = fs::read(dir.path().join("halo.mp3")).unwrap();
    let rename = TrackEdits {
        title: Some("Nope".into()),
        ..TrackEdits::default()
    };

    let unknown = library.edit_tracks(&[halo, 9_999], &rename);
    let rating = library.edit_tracks(
        &[halo],
        &TrackEdits {
            rating: Some(30),
            ..rename.clone()
        },
    );
    let artwork = library.edit_tracks(
        &[halo],
        &TrackEdits {
            artwork: Some(Some(b"definitely not a picture".to_vec())),
            ..rename.clone()
        },
    );

    assert!(
        matches!(unknown, Err(Error::TrackNotFound(9_999))),
        "{unknown:?}"
    );
    assert!(matches!(rating, Err(Error::InvalidRating(30))));
    assert!(
        matches!(artwork, Err(Error::InvalidArtwork(_))),
        "{artwork:?}"
    );
    assert_eq!(library.track(halo).unwrap().unwrap(), before);
    assert_eq!(fs::read(dir.path().join("halo.mp3")).unwrap(), bytes);
    let empty = library.edit_tracks(&[], &rename).unwrap();
    assert!(empty.written.is_empty());
}

#[test]
fn edits_flow_into_search_grouping_and_sorting() {
    let (mut library, _dir) = scanned_library();
    let ids: Vec<i64> = ["Come Together", "Something"]
        .iter()
        .map(|t| track_titled(&library, t).id)
        .collect();

    library
        .edit_tracks(
            &ids,
            &TrackEdits {
                album: Some("The Reissue".into()),
                album_artist: Some("Zed".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    let found = library
        .tracks(&TrackQuery {
            search: Some("reissue".into()),
            ..TrackQuery::default()
        })
        .unwrap();
    assert_eq!(found.len(), 2);
    let albums = library.albums(&TrackQuery::default()).unwrap();
    let reissue = albums.iter().find(|a| a.album == "The Reissue").unwrap();
    assert_eq!(
        (reissue.album_artist.as_str(), reissue.track_count),
        ("Zed", 2)
    );
    assert!(albums.iter().all(|a| a.album != "Abbey Road"));
    let _ = library_of;
}
