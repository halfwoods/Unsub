use std::time::{SystemTime, UNIX_EPOCH};

use antistreaming_library::{Library, SortKey, TrackQuery};
use tempfile::TempDir;

use crate::common::{copy_fixtures, rewind_schema, scan, scanned_library, track_titled};

const DAY: i64 = 86_400;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn plays_and_skips_are_logged_but_only_plays_are_listed_newest_first() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    let zombie = track_titled(&library, "Zombie").id;

    library.record_play(halo, 1_000).unwrap();
    library.record_play(zombie, 2_000).unwrap();
    library.record_skip(halo, 2_500).unwrap();
    library.record_play(halo, 3_000).unwrap();
    library.record_play(zombie, 3_000).unwrap();

    assert_eq!(
        library.recent_plays(0).unwrap(),
        [
            (zombie, 3_000),
            (halo, 3_000),
            (zombie, 2_000),
            (halo, 1_000)
        ],
        "newest first; ties put the later entry first"
    );
    assert_eq!(
        library.recent_plays(2_000).unwrap().len(),
        3,
        "since is inclusive"
    );
    assert_eq!(library.recent_plays(3_001).unwrap(), []);
    let halo_row = library.track(halo).unwrap().unwrap();
    assert_eq!(
        (halo_row.play_count, halo_row.skip_count),
        (2, 1),
        "the counters still count"
    );
}

#[test]
fn a_failed_record_leaves_no_log_entry() {
    let (mut library, _dir) = scanned_library();

    assert!(library.record_play(i64::MAX, 5).is_err());
    assert!(library.record_skip(i64::MAX, 5).is_err());

    assert!(library.recent_plays(0).unwrap().is_empty());
}

#[test]
fn removing_a_track_removes_its_history() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    let zombie = track_titled(&library, "Zombie").id;
    library.record_play(halo, 10).unwrap();
    library.record_play(zombie, 20).unwrap();

    library.remove_tracks(&[halo]).unwrap();

    assert_eq!(library.recent_plays(0).unwrap(), [(zombie, 20)]);
}

#[test]
fn the_recently_played_playlist_follows_the_play_log() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    let zombie = track_titled(&library, "Zombie").id;
    let recently_played = library
        .playlists()
        .unwrap()
        .into_iter()
        .find(|p| p.name == "Recently Played")
        .unwrap()
        .id;
    let listed = |library: &Library| -> Vec<String> {
        library
            .tracks(&TrackQuery {
                kind: None,
                playlist: Some(recently_played),
                sort: SortKey::Name,
                ..TrackQuery::default()
            })
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    };
    assert!(listed(&library).is_empty());

    library.record_play(halo, now() - 3 * DAY).unwrap();
    library.record_play(zombie, now() - 20 * DAY).unwrap();

    assert_eq!(
        listed(&library),
        ["Halo"],
        "three weeks ago is out of the two-week window"
    );
    library.record_play(zombie, now()).unwrap();
    assert_eq!(listed(&library), ["Halo", "Zombie"]);
}

#[test]
fn a_database_from_before_the_play_log_upgrades_with_an_empty_history() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let fixtures = copy_fixtures();
    {
        let mut library = Library::open(&db_path).unwrap();
        scan(&mut library, fixtures.path());
    }
    rewind_schema(&db_path, 3);

    let mut library = Library::open(&db_path).unwrap();

    assert!(library.recent_plays(0).unwrap().is_empty());
    let halo = track_titled(&library, "Halo").id;
    library.record_play(halo, 42).unwrap();
    assert_eq!(library.recent_plays(0).unwrap(), [(halo, 42)]);
}
