use antistreaming_library::{Library, Track};

use crate::common::{copy_fixtures, fixtures_dir, library_of, scan, track};

fn row(id: i64, title: &str, artist: &str, album: &str, duration_ms: u64) -> Track {
    track(id, title, artist, album, duration_ms)
}

fn catalogue() -> Library {
    library_of(&[
        row(1, "Song", "Artist", "Album", 200_000),
        row(
            2,
            "Song (Remastered 2009)",
            "Artist feat. Guest",
            "Album",
            201_500,
        ),
        row(3, "song", "ARTIST", "Other Album", 200_500),
        row(4, "Song (Live)", "Artist", "Album", 200_000),
        row(5, "Song", "Other Artist", "Album", 200_000),
        row(6, "Song", "Artist", "Album", 215_000),
        row(7, "Another", "Band", "Record", 180_000),
        row(8, "Another", "Band", "Record", 180_000),
        row(9, "Unique", "Solo", "Thing", 1_000),
        // Nothing to compare: no artist, no title.
        row(10, "Orphan", "", "", 1_000),
        row(11, "Orphan", "", "", 1_000),
        row(12, "", "Ghost", "", 1_000),
        row(13, "", "Ghost", "", 1_000),
    ])
}

#[test]
fn duplicates_match_on_normalized_title_and_artist() {
    let library = catalogue();

    let groups = library.duplicates(false).unwrap();

    assert_eq!(groups, [vec![1, 2, 3, 6], vec![7, 8]]);
}

#[test]
fn exact_duplicates_also_need_the_same_album_and_a_length_within_two_seconds() {
    let library = catalogue();

    let groups = library.duplicates(true).unwrap();

    // 3 is on another album and 6 is fifteen seconds longer; both drop out.
    assert_eq!(groups, [vec![1, 2], vec![7, 8]]);
}

#[test]
fn exact_groups_never_span_more_than_the_tolerance() {
    let library = library_of(&[
        row(1, "Chain", "Band", "Album", 100_000),
        row(2, "Chain", "Band", "Album", 101_500),
        row(3, "Chain", "Band", "Album", 103_000),
        row(4, "Chain", "Band", "Album", 103_500),
        row(5, "Chain", "Band", "Album", 150_000),
    ]);

    let groups = library.duplicates(true).unwrap();

    // 100.0 s and 101.5 s are within two seconds of each other; 103.0 s is not
    // within two seconds of 100.0 s, so it starts the next group with 103.5 s.
    assert_eq!(groups, [vec![1, 2], vec![3, 4]]);
}

#[test]
fn two_albums_without_a_name_count_as_the_same_album() {
    let library = library_of(&[
        row(1, "Single", "Band", "", 100_000),
        row(2, "Single", "Band", "", 100_000),
        row(3, "Single", "Band", "Named", 100_000),
    ]);

    assert_eq!(library.duplicates(true).unwrap(), [vec![1, 2]]);
    assert_eq!(library.duplicates(false).unwrap(), [vec![1, 2, 3]]);
}

#[test]
fn groups_come_in_order_of_their_lowest_id_with_ids_ascending() {
    let library = library_of(&[
        row(30, "B", "X", "A", 1_000),
        row(10, "A", "X", "A", 1_000),
        row(20, "B", "X", "A", 1_000),
        row(40, "A", "X", "A", 1_000),
    ]);

    assert_eq!(
        library.duplicates(false).unwrap(),
        [vec![10, 40], vec![20, 30]]
    );
}

#[test]
fn no_duplicates_and_an_empty_library_give_no_groups() {
    assert!(
        Library::open_in_memory()
            .unwrap()
            .duplicates(false)
            .unwrap()
            .is_empty()
    );
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    assert!(library.duplicates(false).unwrap().is_empty());
    assert!(library.duplicates(true).unwrap().is_empty());
}

#[test]
fn a_scanned_copy_of_a_file_is_found_in_both_modes() {
    let dir = copy_fixtures();
    std::fs::copy(
        fixtures_dir().join("halo.mp3"),
        dir.path().join("halo copy.mp3"),
    )
    .unwrap();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    let both: Vec<usize> = [false, true]
        .iter()
        .map(|exact| {
            let groups = library.duplicates(*exact).unwrap();
            assert_eq!(groups.len(), 1);
            groups[0].len()
        })
        .collect();

    assert_eq!(both, [2, 2]);
}
