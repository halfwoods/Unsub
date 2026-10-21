use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;
use unsub_library::{Library, SortKey, TrackQuery};

use crate::common::{
    copy_fixtures, library_of, rewind_schema, scan, scanned_library, track, track_titled,
};

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
fn plays_from_elsewhere_count_once_however_often_the_history_is_imported() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    let zombie = track_titled(&library, "Zombie").id;
    library.record_play(halo, 9_000).unwrap();
    let history = [(halo, 1_000), (halo, 2_000), (zombie, 2_000), (halo, 2_000)];

    assert_eq!(library.record_plays_from("spotify", &history).unwrap(), 3);
    assert_eq!(library.record_plays_from("spotify", &history).unwrap(), 0);

    let (halo_row, zombie_row) = (
        library.track(halo).unwrap().unwrap(),
        library.track(zombie).unwrap().unwrap(),
    );
    assert_eq!(
        (halo_row.play_count, halo_row.last_played),
        (3, Some(9_000))
    );
    assert_eq!(
        (zombie_row.play_count, zombie_row.last_played),
        (1, Some(2_000))
    );
    assert_eq!(
        library.recent_plays(0).unwrap(),
        [(halo, 9_000), (zombie, 2_000), (halo, 2_000), (halo, 1_000)]
    );
}

#[test]
fn the_player_itself_can_log_the_moment_a_service_did() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;

    assert_eq!(
        library
            .record_plays_from("spotify", &[(halo, 500)])
            .unwrap(),
        1
    );
    library.record_play(halo, 500).unwrap();
    library.record_play(halo, 500).unwrap();

    assert_eq!(library.track(halo).unwrap().unwrap().play_count, 3);
}

#[test]
fn two_services_recording_one_listen_count_it_once_whichever_came_first() {
    for (first, second) in [("spotify", "listenbrainz"), ("listenbrainz", "spotify")] {
        let mut library = library_of(&[track(1, "Song", "Band", "Album", 200_000)]);

        assert_eq!(
            library
                .record_plays_from(first, &[(1, 1_000), (1, 5_000)])
                .unwrap(),
            2
        );
        // One stamps the start and the other the end, so up to the song's length plus two
        // minutes (320 seconds here) apart is the same listen; any further is another.
        let other = [
            (1, 1_000 - 200),
            (1, 5_000 + 320),
            (1, 5_000 + 321),
            (1, 9_000),
        ];
        assert_eq!(
            library.record_plays_from(second, &other).unwrap(),
            2,
            "{first} then {second}"
        );
        assert_eq!(library.record_plays_from(second, &other).unwrap(), 0);
        assert_eq!(
            library
                .record_plays_from(first, &[(1, 1_000), (1, 5_000)])
                .unwrap(),
            0
        );

        assert_eq!(library.track(1).unwrap().unwrap().play_count, 4);
    }
}

#[test]
fn one_listen_on_another_service_is_one_of_several_real_plays_not_all_of_them() {
    for streaming_first in [true, false] {
        let mut library = library_of(&[track(1, "Song", "Band", "Album", 200_000)]);
        // A full play, and a replay 45 seconds on that Spotify records too; ListenBrainz
        // recorded the first listen only.
        let spotify = [(1, 1_000), (1, 1_045)];
        let listenbrainz = [(1, 1_010)];

        if streaming_first {
            library.record_plays_from("spotify", &spotify).unwrap();
            library
                .record_plays_from("listenbrainz", &listenbrainz)
                .unwrap();
        } else {
            library
                .record_plays_from("listenbrainz", &listenbrainz)
                .unwrap();
            library.record_plays_from("spotify", &spotify).unwrap();
        }

        assert_eq!(
            library.track(1).unwrap().unwrap().play_count,
            2,
            "spotify first: {streaming_first}"
        );
        // And again: nothing is counted that was absorbed, or absorbed that was counted.
        assert_eq!(library.record_plays_from("spotify", &spotify).unwrap(), 0);
        assert_eq!(
            library
                .record_plays_from("listenbrainz", &listenbrainz)
                .unwrap(),
            0
        );
        assert_eq!(library.track(1).unwrap().unwrap().play_count, 2);
    }
}

#[test]
fn a_service_with_every_play_counts_them_all_beside_another_that_has_each_once() {
    let mut library = library_of(&[track(1, "Song", "Band", "Album", 200_000)]);
    let plays = [(1, 1_000), (1, 1_045), (1, 1_090)];

    assert_eq!(library.record_plays_from("spotify", &plays).unwrap(), 3);
    assert_eq!(
        library.record_plays_from("listenbrainz", &plays).unwrap(),
        0
    );
    // A fourth that only ListenBrainz has is its own play.
    assert_eq!(
        library
            .record_plays_from("listenbrainz", &[(1, 1_135)])
            .unwrap(),
        1
    );

    assert_eq!(library.track(1).unwrap().unwrap().play_count, 4);
}

#[test]
fn what_a_service_absorbed_is_counted_again_once_the_play_it_matched_is_gone() {
    let mut library = library_of(&[
        track(1, "Song", "Band", "Album", 200_000),
        track(2, "Song", "Band", "Other", 200_000),
    ]);
    library.record_plays_from("spotify", &[(1, 1_000)]).unwrap();
    assert_eq!(
        library
            .record_plays_from("listenbrainz", &[(2, 1_000)])
            .unwrap(),
        0
    );

    library.remove_tracks(&[1]).unwrap();

    assert_eq!(
        library
            .record_plays_from("listenbrainz", &[(2, 1_000)])
            .unwrap(),
        1
    );
    assert_eq!(library.track(2).unwrap().unwrap().play_count, 1);
}

#[test]
fn a_service_may_repeat_a_song_within_its_own_length() {
    let mut library = library_of(&[track(1, "Song", "Band", "Album", 200_000)]);

    assert_eq!(
        library
            .record_plays_from("spotify", &[(1, 1_000), (1, 1_100)])
            .unwrap(),
        2
    );
}

#[test]
fn a_listen_counts_once_when_the_song_is_matched_to_another_copy_later() {
    let mut library = library_of(&[
        track(1, "Song", "Band", "Greatest Hits", 200_000),
        track(2, "Song (Remastered)", "Band feat. Guest", "Album", 200_000),
    ]);

    assert_eq!(
        library.record_plays_from("spotify", &[(1, 1_000)]).unwrap(),
        1
    );
    // After buying the album, the same play matches that copy instead.
    assert_eq!(
        library.record_plays_from("spotify", &[(2, 1_000)]).unwrap(),
        0
    );

    assert_eq!(library.track(1).unwrap().unwrap().play_count, 1);
    assert_eq!(library.track(2).unwrap().unwrap().play_count, 0);
}

#[test]
fn an_import_with_an_unknown_track_adds_nothing() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;

    assert!(
        library
            .record_plays_from("spotify", &[(halo, 1), (i64::MAX, 2)])
            .is_err()
    );

    assert!(library.recent_plays(0).unwrap().is_empty());
    assert_eq!(library.track(halo).unwrap().unwrap().play_count, 0);
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
