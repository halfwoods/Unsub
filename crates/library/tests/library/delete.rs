use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tempfile::TempDir;
use unsub_library::{Error, Library, PlaylistKind, TrackQuery};

use crate::common::{all_kinds, copy_fixtures, scan, track_titled};

/// A Trash that is just a folder, so tests never touch the real one.
fn fake_trash(library: &mut Library, trash: &Path) -> Arc<Mutex<Vec<PathBuf>>> {
    let trashed = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&trashed);
    let trash = trash.to_path_buf();
    library.set_trash_handler(move |path| {
        fs::create_dir_all(&trash)?;
        fs::rename(path, trash.join(path.file_name().unwrap()))?;
        record.lock().unwrap().push(path.to_path_buf());
        Ok(())
    });
    trashed
}

#[test]
fn deleting_with_trash_moves_the_files_and_removes_the_tracks() {
    let dir = copy_fixtures();
    let bin = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();
    let trashed = fake_trash(&mut library, bin.path());
    scan(&mut library, dir.path());
    let halo = track_titled(&library, "Halo");
    let zombie = track_titled(&library, "Zombie");
    let playlist = library
        .create_playlist("Mine", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library
        .add_to_playlist(playlist, &[halo.id, zombie.id], None)
        .unwrap();
    library.record_play(halo.id, 5).unwrap();

    library
        .delete_tracks(&[halo.id, zombie.id, 9_999], true)
        .unwrap();

    assert_eq!(library.tracks(&all_kinds()).unwrap().len(), 6);
    assert!(library.track(halo.id).unwrap().is_none());
    assert!(!Path::new(&halo.path).exists());
    assert!(bin.path().join("halo.mp3").exists() && bin.path().join("zombie.ogg").exists());
    assert_eq!(trashed.lock().unwrap().len(), 2);
    let listing = TrackQuery {
        playlist: Some(playlist),
        kind: None,
        ..TrackQuery::default()
    };
    assert!(
        library.tracks(&listing).unwrap().is_empty(),
        "gone from playlists too"
    );
    assert!(library.recent_plays(0).unwrap().is_empty());
}

#[test]
fn deleting_without_trash_only_forgets_the_tracks() {
    let dir = copy_fixtures();
    let bin = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();
    let trashed = fake_trash(&mut library, bin.path());
    scan(&mut library, dir.path());
    let halo = track_titled(&library, "Halo");

    library.delete_tracks(&[halo.id], false).unwrap();

    assert!(library.track(halo.id).unwrap().is_none());
    assert!(Path::new(&halo.path).exists(), "the file stays");
    assert!(
        trashed.lock().unwrap().is_empty(),
        "the Trash was never asked"
    );
}

#[test]
fn a_file_that_cannot_be_trashed_keeps_its_track_and_the_rest_go() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    library.set_trash_handler(|path| {
        if path.file_name().is_some_and(|name| name == "zombie.ogg") {
            Err(io::Error::other("the Trash is full"))
        } else {
            Ok(())
        }
    });
    scan(&mut library, dir.path());
    let halo = track_titled(&library, "Halo").id;
    let zombie = track_titled(&library, "Zombie").id;
    let something = track_titled(&library, "Something").id;

    let result = library.delete_tracks(&[halo, zombie, something], true);

    match result {
        Err(Error::TrashFailed(failures)) => {
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].0, zombie);
            assert!(failures[0].1.contains("full"));
        }
        other => panic!("expected a trash failure, got {other:?}"),
    }
    assert!(
        library.track(zombie).unwrap().is_some(),
        "still in the library, since its file is still there"
    );
    assert!(library.track(halo).unwrap().is_none());
    assert!(library.track(something).unwrap().is_none());
}

#[test]
fn a_file_already_gone_is_just_forgotten() {
    let dir = copy_fixtures();
    let bin = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();
    let trashed = fake_trash(&mut library, bin.path());
    scan(&mut library, dir.path());
    let halo = track_titled(&library, "Halo");
    fs::remove_file(&halo.path).unwrap();

    library.delete_tracks(&[halo.id], true).unwrap();

    assert!(library.track(halo.id).unwrap().is_none());
    assert!(trashed.lock().unwrap().is_empty());
}

#[test]
fn emptied_folders_inside_the_media_folder_go_with_their_last_file() {
    let root = TempDir::new().unwrap();
    let media = root.path().join("media");
    let bin = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();
    let file = media.join("Artist/Album/01 Song.mp3");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::copy(crate::common::fixtures_dir().join("halo.mp3"), &file).unwrap();
    library.set_media_folder(&media).unwrap();
    fake_trash(&mut library, bin.path());
    scan(&mut library, &media);
    let id = library.tracks(&all_kinds()).unwrap()[0].id;

    library.delete_tracks(&[id], true).unwrap();

    assert!(media.exists());
    assert!(!media.join("Artist").exists());
}
