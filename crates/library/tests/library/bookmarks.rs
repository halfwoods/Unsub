use std::fs;

use antistreaming_library::{Error, Library, MediaKind, Track, TrackEdits};
use tempfile::TempDir;

use crate::common::{all_kinds, library_of, scan, track};

const HOUR_MS: u64 = 3_600_000;

fn item(id: i64, kind: MediaKind, duration_ms: u64) -> Track {
    Track {
        kind,
        ..track(id, &format!("item {id}"), "Narrator", "Book", duration_ms)
    }
}

fn library() -> Library {
    library_of(&[
        item(1, MediaKind::Audiobook, HOUR_MS),
        item(2, MediaKind::Podcast, HOUR_MS),
        item(3, MediaKind::Music, HOUR_MS),
        item(4, MediaKind::Audiobook, 0),
    ])
}

#[test]
fn audiobooks_and_podcasts_remember_their_place_and_music_does_not() {
    let mut library = library();

    for id in 1..=3 {
        library.set_bookmark(id, 90_000).unwrap();
    }

    assert_eq!(library.bookmark(1).unwrap(), Some(90_000));
    assert_eq!(library.bookmark(2).unwrap(), Some(90_000));
    assert_eq!(library.bookmark(3).unwrap(), None);
    let flags: Vec<bool> = (1..=3)
        .map(|id| library.track(id).unwrap().unwrap().remember_position)
        .collect();
    assert_eq!(flags, [true, true, false]);
}

#[test]
fn the_position_follows_the_latest_value_and_can_be_cleared() {
    let mut library = library();
    library.set_bookmark(1, 90_000).unwrap();
    library.set_bookmark(1, 120_500).unwrap();
    assert_eq!(library.bookmark(1).unwrap(), Some(120_500));

    library.clear_bookmark(1).unwrap();

    assert_eq!(library.bookmark(1).unwrap(), None);
    library.clear_bookmark(1).unwrap();
    library.clear_bookmark(9_999).unwrap();
}

#[test]
fn reaching_the_last_ten_seconds_or_the_start_forgets_the_place() {
    let mut library = library();
    let end = HOUR_MS;

    library.set_bookmark(1, 600_000).unwrap();
    library.set_bookmark(1, end - 10_001).unwrap();
    assert_eq!(
        library.bookmark(1).unwrap(),
        Some(end - 10_001),
        "just outside the last ten seconds"
    );
    library.set_bookmark(1, end - 10_000).unwrap();
    assert_eq!(
        library.bookmark(1).unwrap(),
        None,
        "exactly ten seconds from the end"
    );

    library.set_bookmark(1, 600_000).unwrap();
    library.set_bookmark(1, end).unwrap();
    assert_eq!(library.bookmark(1).unwrap(), None, "finished");

    library.set_bookmark(1, 600_000).unwrap();
    library.set_bookmark(1, end + 5_000).unwrap();
    assert_eq!(library.bookmark(1).unwrap(), None, "past the end");

    library.set_bookmark(1, 600_000).unwrap();
    library.set_bookmark(1, 0).unwrap();
    assert_eq!(library.bookmark(1).unwrap(), None, "back at the start");
}

#[test]
fn a_track_of_unknown_length_keeps_whatever_it_is_given() {
    let mut library = library();

    library.set_bookmark(4, 5_000_000).unwrap();

    assert_eq!(library.bookmark(4).unwrap(), Some(5_000_000));
}

#[test]
fn unknown_tracks_are_an_error_to_set_and_nothing_to_read() {
    let mut library = library();

    assert!(matches!(
        library.set_bookmark(9_999, 5_000),
        Err(Error::TrackNotFound(9_999))
    ));
    assert_eq!(library.bookmark(9_999).unwrap(), None);
}

#[test]
fn get_info_can_turn_remembering_on_or_off_for_a_track() {
    let mut library = library();
    library.set_bookmark(1, 60_000).unwrap();

    let off = TrackEdits {
        remember_position: Some(false),
        ..TrackEdits::default()
    };
    let report = library.edit_tracks(&[1, 2], &off).unwrap();

    assert!(
        report.written.is_empty() && report.db_only.is_empty(),
        "a library setting, not a tag"
    );
    assert_eq!(
        library.bookmark(1).unwrap(),
        None,
        "turning it off forgets the place"
    );
    assert!(!library.track(1).unwrap().unwrap().remember_position);
    library.set_bookmark(1, 70_000).unwrap();
    library.set_bookmark(2, 70_000).unwrap();
    assert_eq!(
        (library.bookmark(1).unwrap(), library.bookmark(2).unwrap()),
        (None, None)
    );

    let on = TrackEdits {
        remember_position: Some(true),
        ..TrackEdits::default()
    };
    library.edit_tracks(&[1, 3], &on).unwrap();
    library.set_bookmark(1, 80_000).unwrap();
    library.set_bookmark(3, 80_000).unwrap();
    assert_eq!(library.bookmark(1).unwrap(), Some(80_000));
    assert_eq!(
        library.bookmark(3).unwrap(),
        Some(80_000),
        "even music, when asked"
    );
    assert!(library.track(3).unwrap().unwrap().remember_position);
    // Editing something else leaves the setting and the place alone.
    library
        .edit_tracks(
            &[1],
            &TrackEdits {
                rating: Some(60),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    assert_eq!(library.bookmark(1).unwrap(), Some(80_000));
}

#[test]
fn changing_the_kind_changes_the_default() {
    let mut library = library();
    library.set_bookmark(1, 60_000).unwrap();

    library
        .edit_tracks(
            &[1, 3],
            &TrackEdits {
                kind: Some(MediaKind::Music),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    assert_eq!(library.bookmark(1).unwrap(), None, "music does not resume");
    library
        .edit_tracks(
            &[3],
            &TrackEdits {
                kind: Some(MediaKind::Audiobook),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    library.set_bookmark(3, 30_000).unwrap();
    assert_eq!(library.bookmark(3).unwrap(), Some(30_000));
}

#[test]
fn a_place_survives_edits_and_rescans_and_is_judged_by_the_files_real_length() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("long.m4b");
    fs::copy(
        crate::common::fixtures_dir().join("audiobook/long.m4b"),
        &path,
    )
    .unwrap();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let book = library.tracks(&all_kinds()).unwrap().remove(0);
    assert_eq!(book.kind, MediaKind::Audiobook);
    assert!(book.remember_position);
    assert!(book.duration_ms > 39_000, "{}", book.duration_ms);
    library.set_bookmark(book.id, 20_000).unwrap();

    library
        .edit_tracks(
            &[book.id],
            &TrackEdits {
                album: Some("Elsewhere".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(600))
        .unwrap();
    library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(library.bookmark(book.id).unwrap(), Some(20_000));
    library
        .set_bookmark(book.id, book.duration_ms - 5_000)
        .unwrap();
    assert_eq!(
        library.bookmark(book.id).unwrap(),
        None,
        "five seconds from the real end"
    );
}

#[test]
fn a_place_is_ignored_if_the_track_turns_out_shorter() {
    let mut library = library();
    library.set_bookmark(1, 1_800_000).unwrap();

    // The file is replaced by a much shorter recording; the old place is now its end.
    library
        .import_tracks(&[item(1, MediaKind::Audiobook, 1_805_000)])
        .unwrap();

    assert_eq!(library.bookmark(1).unwrap(), None);
}

#[test]
fn bookmarks_follow_their_track_out_of_the_library() {
    let mut library = library();
    library.set_bookmark(1, 60_000).unwrap();

    library.remove_tracks(&[1]).unwrap();

    assert_eq!(library.bookmark(1).unwrap(), None);
}
