use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use antistreaming_library::{Error, Library, TrackEdits, TrackOverrides};
use tempfile::TempDir;

use crate::common::{added_ids, all_kinds, copy_fixtures, fixtures_dir, scan, track_titled};

struct Setup {
    _root: TempDir,
    media: PathBuf,
    incoming: PathBuf,
}

fn setup() -> Setup {
    let root = TempDir::new().unwrap();
    let media = root.path().join("media");
    let incoming = root.path().join("incoming");
    fs::create_dir_all(&incoming).unwrap();
    Setup {
        media: media.clone(),
        incoming,
        _root: root,
    }
}

fn library_with_media(setup: &Setup) -> Library {
    let mut library = Library::open_in_memory().unwrap();
    library.set_media_folder(&setup.media).unwrap();
    library
}

fn copy_fixture(fixture: &str, to: &Path) -> PathBuf {
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    fs::copy(fixtures_dir().join(fixture), to).unwrap();
    fs::canonicalize(to).unwrap()
}

fn paths_in(library: &Library, media: &Path) -> Vec<String> {
    let media = fs::canonicalize(media).unwrap();
    let mut paths: Vec<String> = library
        .tracks(&all_kinds())
        .unwrap()
        .into_iter()
        .map(|t| {
            Path::new(&t.path)
                .strip_prefix(&media)
                .map_or(t.path.clone(), |p| p.to_str().unwrap().to_owned())
        })
        .collect();
    paths.sort();
    paths
}

fn files_under(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                out.push(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn settings_default_off_and_persist() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let media = dir.path().join("deep/media");
    {
        let mut library = Library::open(&db_path).unwrap();
        let defaults = library.media_settings().unwrap();
        assert_eq!(
            (defaults.folder, defaults.organize, defaults.copy_on_add),
            (None, false, false)
        );

        library.set_media_folder(&media).unwrap();
        library.set_organize(true).unwrap();
        library.set_copy_on_add(true).unwrap();
    }

    let settings = Library::open(&db_path).unwrap().media_settings().unwrap();

    assert_eq!(settings.folder, Some(fs::canonicalize(&media).unwrap()));
    assert!(media.is_dir(), "the folder is created");
    assert!(settings.organize && settings.copy_on_add);
    let mut library = Library::open(&db_path).unwrap();
    library.set_organize(false).unwrap();
    assert!(!library.media_settings().unwrap().organize);
    assert_eq!(
        serde_json::to_value(library.media_settings().unwrap()).unwrap()["copyOnAdd"],
        true
    );
}

#[test]
fn organize_now_files_every_track_in_the_media_folder_by_its_tags() {
    let s = setup();
    for fixture in [
        "come_together.flac",
        "something.m4a",
        "halo.mp3",
        "bohemian_rhapsody.m4a",
        "zombie.ogg",
        "untagged.wav",
        "chapter_1.m4b",
        "episode_12.mp3",
    ] {
        copy_fixture(fixture, &s.media.join("flat").join(fixture));
    }
    let mut library = library_with_media(&s);
    scan(&mut library, &s.media);
    let ids_before: Vec<i64> = library
        .tracks(&all_kinds())
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    let mut progress = Vec::new();

    let report = library
        .organize_now(|p| progress.push((p.done, p.total)))
        .unwrap();

    assert_eq!(
        (report.moved, report.unchanged, report.left_alone),
        (8, 0, 0)
    );
    assert!(report.failed.is_empty());
    assert_eq!(progress.len(), 8);
    assert_eq!(progress.last(), Some(&(8, 8)));
    assert!(progress.windows(2).all(|w| w[0].0 < w[1].0));
    assert_eq!(
        paths_in(&library, &s.media),
        [
            "Compilations/I Am... Sasha Fierce/2-03 Halo.mp3",
            "Jane Austen/Pride and Prejudice/Chapter 1.m4b",
            "Queen/A Night at the Opera/11 Bohemian Rhapsody.m4a",
            "Radiolab/Radiolab/Episode 12.mp3",
            "The Beatles/Abbey Road/01 Come Together.flac",
            "The Beatles/Abbey Road/02 Something.m4a",
            "The Cranberries/No Need to Argue/02 Zombie.ogg",
            "Unknown Artist/Unknown Album/untagged.wav",
        ]
    );
    assert_eq!(
        files_under(&s.media).len(),
        8,
        "nothing was lost or duplicated"
    );
    assert!(
        !s.media.join("flat").exists(),
        "the emptied folder is removed"
    );
    let ids_after: Vec<i64> = library
        .tracks(&all_kinds())
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(
        ids_before.iter().collect::<std::collections::BTreeSet<_>>(),
        ids_after.iter().collect::<std::collections::BTreeSet<_>>()
    );
    // Every row points at a real file.
    assert!(library.check_missing().unwrap().is_empty());

    let again = library.organize_now(|_| {}).unwrap();
    assert_eq!((again.moved, again.unchanged), (0, 8));
}

#[test]
fn files_outside_the_media_folder_are_never_touched() {
    let s = setup();
    let outside = copy_fixture("halo.mp3", &s.incoming.join("Messy Name.mp3"));
    let inside = copy_fixture("zombie.ogg", &s.media.join("junk/z.ogg"));
    let mut library = library_with_media(&s);
    scan(&mut library, &s.incoming);
    scan(&mut library, &s.media);
    library.set_organize(true).unwrap();

    let report = library.organize_now(|_| {}).unwrap();

    assert_eq!((report.moved, report.left_alone), (1, 1));
    assert!(outside.exists(), "left exactly where it was");
    assert!(!inside.exists());
    // Editing the outside track does not pull it in either.
    let id = track_titled(&library, "Halo").id;
    let edited = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                album: Some("Elsewhere".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    assert!(edited.moved.is_empty() && edited.move_failed.is_empty());
    assert!(outside.exists());
}

#[test]
fn clashing_names_get_numbers_and_never_overwrite() {
    let s = setup();
    for n in 0..3 {
        copy_fixture("halo.mp3", &s.media.join(format!("dupes/halo{n}.mp3")));
    }
    // An unrelated file already sits where the first would go.
    fs::create_dir_all(s.media.join("Compilations/I Am... Sasha Fierce")).unwrap();
    fs::write(
        s.media
            .join("Compilations/I Am... Sasha Fierce/2-03 Halo.mp3"),
        b"somebody else's",
    )
    .unwrap();
    let mut library = library_with_media(&s);
    scan(&mut library, &s.media.join("dupes"));

    let report = library.organize_now(|_| {}).unwrap();

    assert_eq!(report.moved, 3);
    assert_eq!(
        paths_in(&library, &s.media),
        [
            "Compilations/I Am... Sasha Fierce/2-03 Halo 2.mp3",
            "Compilations/I Am... Sasha Fierce/2-03 Halo 3.mp3",
            "Compilations/I Am... Sasha Fierce/2-03 Halo 4.mp3",
        ]
    );
    assert_eq!(
        fs::read(
            s.media
                .join("Compilations/I Am... Sasha Fierce/2-03 Halo.mp3")
        )
        .unwrap(),
        b"somebody else's"
    );
    // Running again does not shuffle the numbers.
    let before = paths_in(&library, &s.media);
    assert_eq!(library.organize_now(|_| {}).unwrap().moved, 0);
    assert_eq!(paths_in(&library, &s.media), before);
}

#[test]
fn organizing_needs_a_media_folder_and_skips_missing_and_cloud_files() {
    let s = setup();
    let mut library = Library::open_in_memory().unwrap();
    assert!(matches!(
        library.organize_now(|_| {}),
        Err(Error::NoMediaFolder)
    ));

    library.set_media_folder(&s.media).unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());
    let gone = copy_fixture("halo.mp3", &s.media.join("a/gone.mp3"));
    let cloud = copy_fixture("zombie.ogg", &s.media.join("b/cloud.ogg"));
    fs::write(cloud.with_extension("evicted"), b"").unwrap();
    scan(&mut library, &s.media);
    fs::remove_file(&gone).unwrap();
    library.check_missing().unwrap();

    let report = library.organize_now(|_| {}).unwrap();

    assert_eq!((report.moved, report.left_alone), (0, 2));
    assert!(cloud.exists());
}

#[test]
fn copy_on_add_copies_files_in_and_leaves_the_originals() {
    let s = setup();
    let original = copy_fixture("halo.mp3", &s.incoming.join("whatever.mp3"));
    copy_fixture("zombie.ogg", &s.incoming.join("sub/other.ogg"));
    let mut library = library_with_media(&s);
    library.set_copy_on_add(true).unwrap();

    let summary = library
        .add_paths(std::slice::from_ref(&s.incoming), |_| {})
        .unwrap();

    assert_eq!((summary.added, summary.failed.len()), (2, 0));
    assert!(original.exists(), "originals stay");
    assert_eq!(
        paths_in(&library, &s.media),
        [
            "Compilations/I Am... Sasha Fierce/2-03 Halo.mp3",
            "The Cranberries/No Need to Argue/02 Zombie.ogg",
        ]
    );
    // The recorded size and time are the copy's, so scanning the media folder finds nothing new.
    let rescan = library
        .add_paths(std::slice::from_ref(&s.media), |_| {})
        .unwrap();
    assert_eq!((rescan.unchanged, rescan.added, rescan.updated), (2, 0, 0));
    assert_eq!(
        track_titled(&library, "Halo").file_size,
        fs::metadata(
            s.media
                .join("Compilations/I Am... Sasha Fierce/2-03 Halo.mp3")
        )
        .unwrap()
        .len()
    );
}

#[test]
fn without_copy_on_add_files_stay_where_they_are() {
    let s = setup();
    let original = copy_fixture("halo.mp3", &s.incoming.join("whatever.mp3"));
    let mut library = library_with_media(&s);
    library.set_organize(true).unwrap();

    library
        .add_paths(std::slice::from_ref(&s.incoming), |_| {})
        .unwrap();

    assert_eq!(
        track_titled(&library, "Halo").path,
        original.to_str().unwrap()
    );
    assert!(files_under(&s.media).is_empty());
}

#[test]
fn keeping_organized_tidies_files_already_in_the_media_folder_as_they_are_added() {
    let s = setup();
    copy_fixture("halo.mp3", &s.media.join("dump/halo.mp3"));
    let mut library = library_with_media(&s);
    library.set_organize(true).unwrap();

    library
        .add_paths(std::slice::from_ref(&s.media), |_| {})
        .unwrap();

    assert_eq!(
        paths_in(&library, &s.media),
        ["Compilations/I Am... Sasha Fierce/2-03 Halo.mp3"]
    );
    assert_eq!(
        files_under(&s.media),
        ["Compilations/I Am... Sasha Fierce/2-03 Halo.mp3"]
    );
}

#[test]
fn add_files_copies_in_once_per_file() {
    let s = setup();
    let download = copy_fixture("episode_12.mp3", &s.incoming.join("episode.mp3"));
    let mut library = library_with_media(&s);
    library.set_copy_on_add(true).unwrap();

    let ids = added_ids(
        &mut library,
        &[
            (download.clone(), TrackOverrides::default()),
            (download.clone(), TrackOverrides::default()),
        ],
    );

    assert_eq!(ids[0], ids[1]);
    assert_eq!(files_under(&s.media), ["Radiolab/Radiolab/Episode 12.mp3"]);
    assert!(download.exists());
}

#[test]
fn editing_moves_the_file_to_follow_its_new_details() {
    let s = setup();
    let mut library = library_with_media(&s);
    library.set_organize(true).unwrap();
    copy_fixture("halo.mp3", &s.media.join("dump/halo.mp3"));
    copy_fixture("something.m4a", &s.media.join("dump/something.m4a"));
    library
        .add_paths(std::slice::from_ref(&s.media), |_| {})
        .unwrap();
    let halo = track_titled(&library, "Halo").id;
    let something = track_titled(&library, "Something").id;
    let playlist = library
        .create_playlist("Mine", antistreaming_library::PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library.add_to_playlist(playlist, &[halo], None).unwrap();
    library.record_play(halo, 5).unwrap();

    let report = library
        .edit_tracks(
            &[halo],
            &TrackEdits {
                title: Some("Halo (Reprise)".into()),
                album: Some("Fierce".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(
        (report.written.as_slice(), report.moved.as_slice()),
        (&[halo][..], &[halo][..])
    );
    assert!(report.move_failed.is_empty());
    let row = library.track(halo).unwrap().unwrap();
    assert!(
        row.path
            .ends_with("Compilations/Fierce/2-03 Halo (Reprise).mp3"),
        "{}",
        row.path
    );
    assert!(Path::new(&row.path).exists());
    assert_eq!((row.play_count, row.title.as_str()), (1, "Halo (Reprise)"));
    assert!(
        files_under(&s.media)
            .iter()
            .any(|f| f == "Compilations/Fierce/2-03 Halo (Reprise).mp3")
    );
    assert!(
        !files_under(&s.media).iter().any(|f| f.contains("I Am")),
        "the old album folder is gone"
    );
    // The move is recorded, so a rescan changes nothing.
    let rescan = library
        .add_paths(std::slice::from_ref(&s.media), |_| {})
        .unwrap();
    assert_eq!((rescan.added, rescan.updated, rescan.unchanged), (0, 0, 2));
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

    // Edits that do not change where it goes, or that only touch the library, leave it alone.
    let before = library.track(something).unwrap().unwrap().path;
    let quiet = library
        .edit_tracks(
            &[something],
            &TrackEdits {
                genre: Some("Rock".into()),
                rating: Some(60),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    assert!(quiet.moved.is_empty());
    assert_eq!(library.track(something).unwrap().unwrap().path, before);
}

#[test]
fn editing_does_not_move_files_when_organizing_is_off() {
    let s = setup();
    let mut library = library_with_media(&s);
    copy_fixture("halo.mp3", &s.media.join("dump/halo.mp3"));
    library
        .add_paths(std::slice::from_ref(&s.media), |_| {})
        .unwrap();
    let id = track_titled(&library, "Halo").id;

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                title: Some("Renamed".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert!(report.moved.is_empty());
    assert!(
        library
            .track(id)
            .unwrap()
            .unwrap()
            .path
            .ends_with("dump/halo.mp3")
    );
}

#[test]
fn a_move_that_fails_is_reported_and_the_edit_still_applies() {
    let s = setup();
    let mut library = library_with_media(&s);
    library.set_organize(true).unwrap();
    copy_fixture(
        "halo.mp3",
        &s.media
            .join("Compilations/I Am... Sasha Fierce/2-03 Halo.mp3"),
    );
    library
        .add_paths(std::slice::from_ref(&s.media), |_| {})
        .unwrap();
    let id = track_titled(&library, "Halo").id;
    // A plain file stands where the new album's folder would have to be created.
    fs::write(s.media.join("Compilations/Blocked"), b"in the way").unwrap();

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                album: Some("Blocked".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert!(report.moved.is_empty());
    assert_eq!(report.move_failed.len(), 1);
    assert_eq!(report.written, [id]);
    let row = library.track(id).unwrap().unwrap();
    assert_eq!(row.album, "Blocked");
    assert!(row.path.ends_with("I Am... Sasha Fierce/2-03 Halo.mp3"));
    assert!(Path::new(&row.path).exists());
}

#[test]
fn a_rescan_does_not_resurrect_a_moved_files_old_row() {
    let s = setup();
    let dir = copy_fixtures();
    let mut library = library_with_media(&s);
    scan(&mut library, dir.path());
    library.set_media_folder(dir.path()).unwrap();
    library.set_organize(true).unwrap();
    library.organize_now(|_| {}).unwrap();
    touch(
        &dir.path()
            .join("The Beatles/Abbey Road/01 Come Together.flac"),
    );

    let rescan = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(rescan.added, 0, "the moved files are the same tracks");
    assert_eq!(library.tracks(&all_kinds()).unwrap().len(), 8);
}

fn touch(path: &Path) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(120))
        .unwrap();
}
