use antistreaming_library::{Error, Library};

use crate::common::{all_kinds, scanned_library, track_titled};

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
