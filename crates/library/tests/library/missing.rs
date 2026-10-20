use std::fs;
use std::path::{Path, PathBuf};

use antistreaming_library::{Error, Library, PlaylistKind};
use tempfile::TempDir;

use crate::common::{all_kinds, copy_fixtures, fixtures_dir, scan, track_titled};

fn moved(dir: &TempDir, from: &str, to: &str) -> PathBuf {
    let new = dir.path().join(to);
    fs::create_dir_all(new.parent().unwrap()).unwrap();
    fs::rename(dir.path().join(from), &new).unwrap();
    fs::canonicalize(new).unwrap()
}

#[test]
fn missing_files_are_flagged_and_cleared_when_they_return() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let halo = track_titled(&library, "Halo");
    let zombie = track_titled(&library, "Zombie");
    assert!(library.check_missing().unwrap().is_empty());
    assert!(
        library
            .tracks(&all_kinds())
            .unwrap()
            .iter()
            .all(|t| !t.missing)
    );

    let aside = dir.path().join("aside.mp3");
    fs::rename(dir.path().join("halo.mp3"), &aside).unwrap();
    fs::remove_file(dir.path().join("zombie.ogg")).unwrap();
    let mut found = library.check_missing().unwrap();
    found.sort_unstable();

    let mut expected = vec![halo.id, zombie.id];
    expected.sort_unstable();
    assert_eq!(found, expected);
    assert!(library.track(halo.id).unwrap().unwrap().missing);
    assert!(library.track(zombie.id).unwrap().unwrap().missing);
    assert_eq!(
        library
            .tracks(&all_kinds())
            .unwrap()
            .iter()
            .filter(|t| t.missing)
            .count(),
        2
    );

    fs::rename(&aside, dir.path().join("halo.mp3")).unwrap();
    assert_eq!(library.check_missing().unwrap(), [zombie.id]);
    assert!(
        !library.track(halo.id).unwrap().unwrap().missing,
        "back, so no longer flagged"
    );
}

#[test]
fn a_rescan_clears_the_flag_on_a_file_that_is_back() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let id = track_titled(&library, "Halo").id;
    let aside = dir.path().join("aside.mp3");
    fs::rename(dir.path().join("halo.mp3"), &aside).unwrap();
    library.check_missing().unwrap();
    assert!(library.track(id).unwrap().unwrap().missing);

    fs::rename(&aside, dir.path().join("halo.mp3")).unwrap();
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(summary.unchanged, 8);
    assert!(!library.track(id).unwrap().unwrap().missing);
}

#[test]
fn cloud_placeholders_are_not_checked() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cloud.mp3");
    fs::write(&path, b"placeholder").unwrap();
    fs::write(path.with_extension("evicted"), b"").unwrap();
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|p| p.with_extension("evicted").exists());
    scan(&mut library, dir.path());
    let id = library.tracks(&all_kinds()).unwrap()[0].id;

    fs::remove_file(&path).unwrap();

    assert!(library.check_missing().unwrap().is_empty());
    assert!(!library.track(id).unwrap().unwrap().missing);
}

#[test]
fn relinking_points_a_track_at_a_new_file_and_keeps_its_history() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let id = track_titled(&library, "Halo").id;
    library.record_play(id, 10).unwrap();
    library.set_rating(id, 80).unwrap();
    let playlist = library
        .create_playlist("Mine", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library.add_to_playlist(playlist, &[id], None).unwrap();
    let new = moved(&dir, "halo.mp3", "somewhere/else/renamed.mp3");
    library.check_missing().unwrap();

    library.relink(id, &new).unwrap();

    let row = library.track(id).unwrap().unwrap();
    assert_eq!(row.path, new.to_str().unwrap());
    assert!(!row.missing);
    assert_eq!(
        (row.title.as_str(), row.play_count, row.rating),
        ("Halo", 1, 80)
    );
    assert_eq!(row.file_size, fs::metadata(&new).unwrap().len());
    assert!(library.check_missing().unwrap().is_empty());
    // The size and time were recorded, so a scan finds it unchanged.
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!(
        (summary.added, summary.updated, summary.unchanged),
        (0, 0, 8)
    );
    assert_eq!(
        library
            .tracks(&antistreaming_library::TrackQuery {
                playlist: Some(playlist),
                kind: None,
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn relinking_rejects_bad_targets_and_changes_nothing() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let halo = track_titled(&library, "Halo");
    let zombie = track_titled(&library, "Zombie");

    let in_use = library.relink(halo.id, Path::new(&zombie.path));
    let missing = library.relink(halo.id, &dir.path().join("nothing.mp3"));
    let folder = library.relink(halo.id, dir.path());
    let unknown = library.relink(9_999, Path::new(&halo.path));

    assert!(matches!(in_use, Err(Error::PathInUse(_))), "{in_use:?}");
    assert!(matches!(missing, Err(Error::Io(_))), "{missing:?}");
    assert!(matches!(folder, Err(Error::Io(_))), "{folder:?}");
    assert!(matches!(unknown, Err(Error::TrackNotFound(9_999))));
    assert_eq!(library.track(halo.id).unwrap().unwrap(), halo);
    library.relink(halo.id, Path::new(&halo.path)).unwrap();
}

#[test]
fn locating_other_missing_files_matches_by_name_and_size_or_tags() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let id = |title: &str| track_titled(&library, title).id;
    let (halo, zombie, something, queen, chapter) = (
        id("Halo"),
        id("Zombie"),
        id("Something"),
        id("Bohemian Rhapsody"),
        id("Chapter 1"),
    );

    // A drive was rearranged: some files moved intact, one was re-encoded (same
    // name and tags, different size), one name now belongs to a different song,
    // and one never came back.
    let found = TempDir::new().unwrap();
    for (from, to) in [
        ("zombie.ogg", "Music/Cranberries/zombie.ogg"),
        ("something.m4a", "Music/Beatles/something.m4a"),
    ] {
        let target = found.path().join(to);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::rename(dir.path().join(from), target).unwrap();
    }
    let mut reencoded = fs::read(fixtures_dir().join("halo.mp3")).unwrap();
    reencoded.extend_from_slice(&[0; 200]);
    fs::create_dir_all(found.path().join("Music/Beyonce")).unwrap();
    fs::write(found.path().join("Music/Beyonce/halo.mp3"), reencoded).unwrap();
    fs::remove_file(dir.path().join("halo.mp3")).unwrap();
    fs::copy(
        fixtures_dir().join("zombie.ogg"),
        found.path().join("Music/bohemian_rhapsody.m4a"),
    )
    .unwrap();
    fs::remove_file(dir.path().join("bohemian_rhapsody.m4a")).unwrap();
    fs::remove_file(dir.path().join("chapter_1.m4b")).unwrap();
    let mut missing = library.check_missing().unwrap();
    missing.sort_unstable();
    assert_eq!(missing.len(), 5);

    let mut relinked = library.relink_folder(found.path()).unwrap();
    relinked.sort_unstable();

    let mut expected = vec![halo, zombie, something];
    expected.sort_unstable();
    assert_eq!(
        relinked, expected,
        "by size for two, by tags for the re-encoded one"
    );
    for id in [halo, zombie, something] {
        let row = library.track(id).unwrap().unwrap();
        assert!(!row.missing);
        assert!(Path::new(&row.path).starts_with(fs::canonicalize(found.path()).unwrap()));
    }
    assert!(
        library.track(queen).unwrap().unwrap().missing,
        "a different song under that name is not it"
    );
    assert!(library.track(chapter).unwrap().unwrap().missing);
    assert_eq!(library.track(halo).unwrap().unwrap().title, "Halo");
}

#[test]
fn each_found_file_relinks_at_most_one_track() {
    let dir = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();
    for folder in ["a", "b"] {
        fs::create_dir_all(dir.path().join(folder)).unwrap();
        fs::copy(
            fixtures_dir().join("episode_12.mp3"),
            dir.path().join(folder).join("episode.mp3"),
        )
        .unwrap();
    }
    scan(&mut library, dir.path());
    assert_eq!(library.tracks(&all_kinds()).unwrap().len(), 2);
    let found = TempDir::new().unwrap();
    fs::copy(
        fixtures_dir().join("episode_12.mp3"),
        found.path().join("episode.mp3"),
    )
    .unwrap();
    fs::remove_dir_all(dir.path().join("a")).unwrap();
    fs::remove_dir_all(dir.path().join("b")).unwrap();
    library.check_missing().unwrap();

    let relinked = library.relink_folder(found.path()).unwrap();

    assert_eq!(relinked.len(), 1, "one file, one track");
    assert_eq!(
        library
            .tracks(&all_kinds())
            .unwrap()
            .iter()
            .filter(|t| t.missing)
            .count(),
        1
    );
    assert!(
        library.relink_folder(found.path()).unwrap().is_empty(),
        "already used"
    );
}

#[test]
fn locating_with_nothing_missing_does_nothing() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    assert!(library.relink_folder(dir.path()).unwrap().is_empty());
    assert!(
        library
            .relink_folder(&dir.path().join("nonexistent"))
            .unwrap()
            .is_empty()
    );
}
