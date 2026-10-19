use std::fs::{self, File};
use std::time::{Duration, SystemTime};

use antistreaming_library::{Library, MediaKind};
use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::id3::v2::{BinaryFrame, Frame, FrameId, TextInformationFrame};
use lofty::mpeg::MpegFile;
use lofty::tag::{Accessor, ItemKey};
use rusqlite::Connection;
use tempfile::TempDir;

use crate::common::{
    FIXTURE_COUNT, MUSIC_COUNT, all_kinds, copy_fixtures, fixtures_dir, scan, scanned_library,
    track_titled,
};

#[test]
fn scan_reports_counts_and_stats() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(summary.scanned, FIXTURE_COUNT);
    assert_eq!(summary.added, FIXTURE_COUNT);
    assert_eq!((summary.updated, summary.unchanged), (0, 0));
    assert!(summary.failed.is_empty());

    let tracks = library.tracks(&all_kinds()).unwrap();
    let stats = library.stats(&all_kinds()).unwrap();
    assert_eq!(stats.count, FIXTURE_COUNT);
    assert_eq!(
        stats.total_bytes,
        tracks.iter().map(|t| t.file_size).sum::<u64>()
    );
    assert_eq!(
        stats.total_duration_ms,
        tracks.iter().map(|t| t.duration_ms).sum::<u64>()
    );
    // Each fixture is a 0.5 s sine, give or take codec padding.
    assert!(
        tracks.iter().all(|t| (400..=700).contains(&t.duration_ms)),
        "{tracks:#?}"
    );

    assert_eq!(
        library.stats(&Default::default()).unwrap().count,
        MUSIC_COUNT as u64
    );
}

#[test]
fn rescan_reports_everything_unchanged() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let before = library.tracks(&all_kinds()).unwrap();

    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(summary.scanned, FIXTURE_COUNT);
    assert_eq!(summary.unchanged, FIXTURE_COUNT);
    assert_eq!((summary.added, summary.updated), (0, 0));
    assert_eq!(library.tracks(&all_kinds()).unwrap(), before);
}

#[test]
fn modified_file_is_updated_and_keeps_user_data() {
    let dir = copy_fixtures();
    let db = TempDir::new().unwrap();
    let db_path = db.path().join("library.db");
    let mut library = Library::open(&db_path).unwrap();
    scan(&mut library, dir.path());
    // Backdate so an overwrite with "now" would be visible.
    Connection::open(&db_path)
        .unwrap()
        .execute("UPDATE tracks SET date_added = 1", [])
        .unwrap();
    let halo = track_titled(&library, "Halo");
    assert_eq!(halo.date_added, 1);
    library.record_play(halo.id, 1_000).unwrap();
    library.set_rating(halo.id, 80).unwrap();

    let path = dir.path().join("halo.mp3");
    let mut tagged = lofty::read_from_path(&path).unwrap();
    tagged
        .primary_tag_mut()
        .unwrap()
        .set_title("Halo (Remaster)".into());
    tagged.save_to_path(&path, WriteOptions::default()).unwrap();

    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!((summary.updated, summary.unchanged), (1, FIXTURE_COUNT - 1));

    let updated = library.track(halo.id).unwrap().unwrap();
    assert_eq!(updated.title, "Halo (Remaster)");
    assert_eq!((updated.play_count, updated.rating), (1, 80));
    assert_eq!(updated.date_added, halo.date_added);
}

#[test]
fn files_read_by_an_older_parser_are_reread_even_if_unchanged() {
    let dir = copy_fixtures();
    let db = TempDir::new().unwrap();
    let db_path = db.path().join("library.db");
    let mut library = Library::open(&db_path).unwrap();
    scan(&mut library, dir.path());
    let halo = track_titled(&library, "Halo");
    library.record_play(halo.id, 5).unwrap();
    let stale = Connection::open(&db_path).unwrap();
    stale
        .execute("UPDATE tracks SET parser_version = 1", [])
        .unwrap();
    // A row written by a newer build than this one is trusted, not re-read.
    stale
        .execute(
            "UPDATE tracks SET parser_version = 999 WHERE title = 'Zombie'",
            [],
        )
        .unwrap();

    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(
        (summary.updated, summary.unchanged),
        (FIXTURE_COUNT - 1, 1),
        "{summary:?}"
    );
    assert_eq!(library.track(halo.id).unwrap().unwrap().play_count, 1);
    let again = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!((again.updated, again.unchanged), (0, FIXTURE_COUNT));
}

#[test]
fn same_size_but_newer_mtime_is_rescanned() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    let file = File::options()
        .write(true)
        .open(dir.path().join("zombie.ogg"))
        .unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(120))
        .unwrap();

    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!((summary.updated, summary.unchanged), (1, FIXTURE_COUNT - 1));
}

#[test]
fn tag_fields_are_read() {
    let (library, dir) = scanned_library();

    let come_together = track_titled(&library, "Come Together");
    assert_eq!(come_together.artist, "The Beatles");
    assert_eq!(come_together.album, "Abbey Road");
    assert_eq!(come_together.album_artist, "The Beatles");
    assert_eq!(come_together.genre, "Rock");
    assert_eq!(come_together.composer, "Lennon-McCartney");
    assert_eq!(come_together.year, Some(1969));
    assert_eq!(
        (come_together.track_number, come_together.track_count),
        (Some(1), Some(17))
    );
    assert_eq!(
        (come_together.disc_number, come_together.disc_count),
        (Some(1), Some(1))
    );
    assert_eq!(come_together.codec, "FLAC");
    assert_eq!(come_together.sample_rate, Some(8000));
    assert_eq!(come_together.channels, Some(1));
    assert_eq!(come_together.bit_depth, Some(16));
    assert_eq!(come_together.kind, MediaKind::Music);
    assert!(!come_together.compilation);
    assert_eq!(come_together.rating, 0);
    assert_eq!((come_together.play_count, come_together.skip_count), (0, 0));
    assert_eq!(come_together.last_played, None);
    assert_eq!(
        come_together.path,
        fs::canonicalize(dir.path().join("come_together.flac"))
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert_eq!(
        come_together.file_size,
        fs::metadata(dir.path().join("come_together.flac"))
            .unwrap()
            .len()
    );

    let halo = track_titled(&library, "Halo");
    assert_eq!(halo.artist, "Beyoncé");
    assert_eq!(halo.album, "I Am... Sasha Fierce");
    assert_eq!((halo.track_number, halo.track_count), (Some(3), Some(12)));
    assert_eq!((halo.disc_number, halo.disc_count), (Some(2), Some(2)));
    assert_eq!(halo.year, Some(2008));
    assert!(halo.compilation);
    assert!(halo.bitrate_kbps.is_some());

    let zombie = track_titled(&library, "Zombie");
    assert_eq!(zombie.artist, "The Cranberries");
    assert_eq!(zombie.album_artist, "");
    assert_eq!((zombie.track_number, zombie.track_count), (Some(2), None));
    assert_eq!(zombie.year, Some(1994));
}

#[test]
fn codecs_are_identified() {
    let (library, _dir) = scanned_library();
    let codec = |title: &str| track_titled(&library, title).codec;

    assert_eq!(codec("Come Together"), "FLAC");
    assert_eq!(codec("Something"), "ALAC");
    assert_eq!(codec("Bohemian Rhapsody"), "AAC");
    assert_eq!(codec("Chapter 1"), "AAC");
    assert_eq!(codec("Halo"), "MP3");
    assert_eq!(codec("Zombie"), "Vorbis");
    assert_eq!(codec("untagged"), "WAV");
}

#[test]
fn untagged_file_falls_back_to_file_stem_with_blank_fields() {
    let (library, _dir) = scanned_library();
    let untagged = track_titled(&library, "untagged");

    assert_eq!(
        (
            untagged.artist.as_str(),
            untagged.album.as_str(),
            untagged.genre.as_str()
        ),
        ("", "", "")
    );
    assert_eq!((untagged.year, untagged.track_number), (None, None));
}

#[test]
fn media_kind_comes_from_extension_genre_and_flags() {
    let (library, _dir) = scanned_library();

    assert_eq!(
        track_titled(&library, "Chapter 1").kind,
        MediaKind::Audiobook
    );
    assert_eq!(
        track_titled(&library, "Episode 12").kind,
        MediaKind::Podcast
    );
    assert_eq!(track_titled(&library, "Halo").kind, MediaKind::Music);
}

#[test]
fn audiobook_genre_matches_case_insensitively() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("book.mp3");
    fs::copy(fixtures_dir().join("halo.mp3"), &path).unwrap();
    let mut tagged = lofty::read_from_path(&path).unwrap();
    tagged
        .primary_tag_mut()
        .unwrap()
        .set_genre("audiobooks".into());
    tagged.save_to_path(&path, WriteOptions::default()).unwrap();

    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    assert_eq!(
        library.tracks(&all_kinds()).unwrap()[0].kind,
        MediaKind::Audiobook
    );
}

#[test]
fn id3_pcst_frame_marks_a_podcast() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("halo.mp3");
    fs::copy(fixtures_dir().join("halo.mp3"), &path).unwrap();
    // iTunes writes PCST as a binary frame holding four zero bytes.
    let mut mpeg =
        MpegFile::read_from(&mut File::open(&path).unwrap(), ParseOptions::new()).unwrap();
    mpeg.id3v2_mut()
        .unwrap()
        .insert(Frame::Binary(BinaryFrame::new(
            FrameId::new("PCST").unwrap(),
            vec![0; 4],
        )));
    mpeg.save_to_path(&path, WriteOptions::default()).unwrap();

    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    assert_eq!(
        library.tracks(&all_kinds()).unwrap()[0].kind,
        MediaKind::Podcast
    );
}

#[test]
fn mp4_pcst_atom_marks_a_podcast() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("something.m4a");
    fs::copy(fixtures_dir().join("something.m4a"), &path).unwrap();
    let mut tagged = lofty::read_from_path(&path).unwrap();
    tagged
        .primary_tag_mut()
        .unwrap()
        .insert_text(ItemKey::FlagPodcast, "1".into());
    tagged.save_to_path(&path, WriteOptions::default()).unwrap();

    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    assert_eq!(
        library.tracks(&all_kinds()).unwrap()[0].kind,
        MediaKind::Podcast
    );
}

#[test]
fn utf8_bytes_in_a_latin1_id3_frame_are_repaired() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("halo.mp3");
    fs::copy(fixtures_dir().join("halo.mp3"), &path).unwrap();
    // Latin-1 frames holding the characters of UTF-8 bytes is how real files
    // end up with "JÃ³zsef"; lofty writes each char as one Latin-1 byte.
    let mut mpeg =
        MpegFile::read_from(&mut File::open(&path).unwrap(), ParseOptions::new()).unwrap();
    for (id, text) in [
        ("TPE1", "Garai J\u{c3}\u{b3}zsef"),
        ("TIT2", "\u{c2}\u{a9} Klankbeeld"),
    ] {
        mpeg.id3v2_mut()
            .unwrap()
            .insert(Frame::Text(TextInformationFrame::new(
                FrameId::new(id).unwrap(),
                TextEncoding::Latin1,
                text,
            )));
    }
    mpeg.save_to_path(&path, WriteOptions::default()).unwrap();
    let bytes = fs::read(&path).unwrap();
    let utf8_bytes = b"Garai J\xC3\xB3zsef";
    assert!(
        bytes
            .windows(utf8_bytes.len())
            .any(|window| window == utf8_bytes),
        "the file must really hold UTF-8 bytes in a Latin-1 frame"
    );

    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    let repaired = &library.tracks(&all_kinds()).unwrap()[0];
    assert_eq!(repaired.artist, "Garai J\u{f3}zsef");
    assert_eq!(repaired.title, "\u{a9} Klankbeeld");
    assert_eq!(repaired.album, "I Am... Sasha Fierce");
}

#[test]
fn hidden_and_non_audio_files_are_skipped() {
    let dir = copy_fixtures();
    let halo = fixtures_dir().join("halo.mp3");
    fs::copy(&halo, dir.path().join("._halo.mp3")).unwrap();
    fs::copy(&halo, dir.path().join(".hidden.mp3")).unwrap();
    fs::create_dir(dir.path().join(".cache")).unwrap();
    fs::copy(&halo, dir.path().join(".cache/halo.mp3")).unwrap();
    fs::write(dir.path().join("notes.txt"), "not audio").unwrap();
    fs::copy(
        fixtures_dir().join("come_together.flac"),
        dir.path().join("LOUD.FLAC"),
    )
    .unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    fs::copy(&halo, dir.path().join("nested/halo_again.MP3")).unwrap();

    let mut library = Library::open_in_memory().unwrap();
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    // The fixtures plus the upper-case and nested copies; the temp root is
    // itself dot-prefixed, which must not exclude it.
    assert_eq!(summary.scanned, FIXTURE_COUNT + 2);
    assert!(summary.failed.is_empty());
}

#[test]
fn overlapping_paths_and_single_files_are_deduplicated() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();

    let summary = library
        .add_paths(
            &[dir.path().to_path_buf(), dir.path().join("halo.mp3")],
            |_| {},
        )
        .unwrap();

    assert_eq!(
        (summary.scanned, summary.added),
        (FIXTURE_COUNT, FIXTURE_COUNT)
    );
}

#[test]
fn unreadable_files_are_reported_without_aborting() {
    let dir = copy_fixtures();
    fs::write(dir.path().join("broken.mp3"), "this is not an mp3").unwrap();
    let mut library = Library::open_in_memory().unwrap();

    let summary = library
        .add_paths(
            &[dir.path().to_path_buf(), dir.path().join("missing")],
            |_| {},
        )
        .unwrap();

    assert_eq!(summary.added, FIXTURE_COUNT);
    assert_eq!(summary.failed.len(), 2, "{:?}", summary.failed);
    assert!(
        summary
            .failed
            .iter()
            .any(|(path, _)| path.ends_with("broken.mp3"))
    );
    assert!(
        summary
            .failed
            .iter()
            .any(|(path, _)| path.ends_with("missing"))
    );
}

#[test]
fn large_scans_commit_in_batches_and_report_progress() {
    let dir = TempDir::new().unwrap();
    let source = fixtures_dir().join("episode_12.mp3");
    let total = 1_100;
    for n in 0..total {
        fs::copy(&source, dir.path().join(format!("copy_{n}.mp3"))).unwrap();
    }
    let mut library = Library::open_in_memory().unwrap();

    let mut reports = Vec::new();
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |p| reports.push(p))
        .unwrap();

    assert_eq!(summary.added, total);
    assert_eq!(library.stats(&all_kinds()).unwrap().count, total);
    assert_eq!(reports.len(), total as usize / 50);
    assert_eq!(reports[0].scanned, 50);
    assert_eq!(reports.last().unwrap().scanned, 1_100);
    assert_eq!(reports.last().unwrap().added, 1_100);
    assert!(reports[0].current_path.ends_with(".mp3"));
}
