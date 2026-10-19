use antistreaming_library::{Error, ExternalStats, Library};
use rusqlite::Connection;
use tempfile::TempDir;

use crate::common::{all_kinds, copy_fixtures, scan, scanned_library, track_titled};

#[test]
fn record_play_and_skip_update_counters() {
    let (mut library, _dir) = scanned_library();
    let id = track_titled(&library, "Halo").id;

    library.record_play(id, 1_700_000_000).unwrap();
    library.record_play(id, 1_700_000_500).unwrap();
    library.record_skip(id, 1_700_000_900).unwrap();

    let halo = library.track(id).unwrap().unwrap();
    assert_eq!(halo.play_count, 2);
    assert_eq!(halo.last_played, Some(1_700_000_500));
    assert_eq!(halo.skip_count, 1);
}

#[test]
fn rating_accepts_itunes_steps_only() {
    let (mut library, _dir) = scanned_library();
    let id = track_titled(&library, "Halo").id;

    for rating in [0, 20, 40, 60, 80, 100] {
        library.set_rating(id, rating).unwrap();
        assert_eq!(library.track(id).unwrap().unwrap().rating, rating);
    }
    for rating in [1, 30, 99, 101, 120, 255] {
        assert!(
            matches!(library.set_rating(id, rating), Err(Error::InvalidRating(r)) if r == rating)
        );
    }
    assert_eq!(library.track(id).unwrap().unwrap().rating, 100);
}

#[test]
fn unknown_track_ids_are_errors() {
    let (mut library, _dir) = scanned_library();

    assert!(matches!(
        library.record_play(-1, 0),
        Err(Error::TrackNotFound(-1))
    ));
    assert!(matches!(
        library.record_skip(-1, 0),
        Err(Error::TrackNotFound(-1))
    ));
    assert!(matches!(
        library.set_rating(-1, 20),
        Err(Error::TrackNotFound(-1))
    ));
}

#[test]
fn remove_tracks_deletes_only_the_given_ids() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    let zombie = track_titled(&library, "Zombie").id;
    let before = library.stats(&all_kinds()).unwrap();

    library.remove_tracks(&[halo, zombie, i64::MAX]).unwrap();

    assert_eq!(library.track(halo).unwrap(), None);
    assert_eq!(library.track(zombie).unwrap(), None);
    assert_eq!(library.stats(&all_kinds()).unwrap().count, before.count - 2);
    assert!(
        library
            .track(track_titled(&library, "Something").id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn removed_tracks_return_when_rescanned() {
    let (mut library, dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    library.remove_tracks(&[halo]).unwrap();

    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!((summary.added, summary.unchanged), (1, 7));
    assert_eq!(track_titled(&library, "Halo").play_count, 0);
}

#[test]
fn empty_removal_is_a_no_op() {
    let mut library = Library::open_in_memory().unwrap();

    library.remove_tracks(&[]).unwrap();
}

#[test]
fn the_song_checkbox_defaults_on_and_survives_rescans() {
    let dir = copy_fixtures();
    let db = TempDir::new().unwrap();
    let db_path = db.path().join("library.db");
    let mut library = Library::open(&db_path).unwrap();
    scan(&mut library, dir.path());
    assert!(
        library
            .tracks(&all_kinds())
            .unwrap()
            .iter()
            .all(|t| t.enabled)
    );
    let halo = track_titled(&library, "Halo").id;
    let zombie = track_titled(&library, "Zombie").id;

    library
        .set_enabled(&[halo, zombie, i64::MAX], false)
        .unwrap();
    library.set_enabled(&[zombie], true).unwrap();

    assert!(!library.track(halo).unwrap().unwrap().enabled);
    assert!(library.track(zombie).unwrap().unwrap().enabled);
    // Force every file to be re-read; the checkbox is the user's, not the file's.
    Connection::open(&db_path)
        .unwrap()
        .execute("UPDATE tracks SET parser_version = 1", [])
        .unwrap();
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!(summary.updated, 8);
    assert!(!library.track(halo).unwrap().unwrap().enabled);
}

fn stats(overrides: impl FnOnce(&mut ExternalStats)) -> ExternalStats {
    let mut stats = ExternalStats {
        play_count: 0,
        skip_count: 0,
        last_played: None,
        last_skipped: None,
        rating: 0,
        date_added: None,
        enabled: true,
    };
    overrides(&mut stats);
    stats
}

#[test]
fn external_stats_merge_without_losing_either_sides_history() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    for n in 0..4 {
        library.record_play(halo, 1_000 + n).unwrap();
    }
    library.record_skip(halo, 5_000).unwrap();
    library.set_rating(halo, 40).unwrap();
    let ours = library.track(halo).unwrap().unwrap();

    // Theirs is behind on counts and older on dates, but has an earlier date added.
    library
        .apply_external_stats(&[(
            halo,
            stats(|s| {
                s.play_count = 2;
                s.skip_count = 3;
                s.last_played = Some(500);
                s.last_skipped = Some(9_000);
                s.rating = 100;
                s.date_added = Some(ours.date_added - 1_000);
                s.enabled = false;
            }),
        )])
        .unwrap();

    let merged = library.track(halo).unwrap().unwrap();
    assert_eq!(merged.play_count, 4, "the larger count");
    assert_eq!(merged.skip_count, 3);
    assert_eq!(merged.last_played, Some(1_003), "the later date");
    assert_eq!(merged.rating, 40, "ours stands when we have one");
    assert_eq!(merged.date_added, ours.date_added - 1_000, "the earliest");
    assert!(!merged.enabled, "the checkbox follows theirs");
}

#[test]
fn external_stats_fill_gaps_and_ignore_missing_values() {
    let (mut library, _dir) = scanned_library();
    let zombie = track_titled(&library, "Zombie");

    library
        .apply_external_stats(&[(
            zombie.id,
            stats(|s| {
                s.play_count = 7;
                s.last_played = Some(2_000);
                s.rating = 80;
            }),
        )])
        .unwrap();

    let filled = library.track(zombie.id).unwrap().unwrap();
    assert_eq!(
        (filled.play_count, filled.last_played, filled.rating),
        (7, Some(2_000), 80)
    );
    assert_eq!(filled.skip_count, 0);
    assert_eq!(
        filled.date_added, zombie.date_added,
        "no date from them leaves ours"
    );
    assert!(filled.enabled);

    library
        .apply_external_stats(&[(zombie.id, stats(|s| s.last_played = None))])
        .unwrap();
    assert_eq!(
        library.track(zombie.id).unwrap().unwrap().last_played,
        Some(2_000)
    );
}

#[test]
fn external_ratings_are_capped_and_a_later_date_added_loses() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo");

    library
        .apply_external_stats(&[(
            halo.id,
            stats(|s| {
                s.rating = 200;
                s.date_added = Some(halo.date_added + 5_000);
            }),
        )])
        .unwrap();

    let merged = library.track(halo.id).unwrap().unwrap();
    assert_eq!(merged.rating, 100);
    assert_eq!(merged.date_added, halo.date_added);
}

#[test]
fn external_stats_apply_all_or_nothing() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo");

    let result = library.apply_external_stats(&[
        (halo.id, stats(|s| s.play_count = 9)),
        (i64::MAX, stats(|_| {})),
    ]);

    assert!(matches!(result, Err(Error::TrackNotFound(id)) if id == i64::MAX));
    assert_eq!(library.track(halo.id).unwrap().unwrap().play_count, 0);
    library.apply_external_stats(&[]).unwrap();
}
