use std::fs;

use tempfile::TempDir;
use unsub_library::Library;

use crate::common::{all_kinds, fixtures_dir, scan, scanned_library, track_titled};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

fn single_track_library(fixture: &str) -> (Library, TempDir, i64) {
    let dir = TempDir::new().unwrap();
    fs::copy(fixtures_dir().join(fixture), dir.path().join(fixture)).unwrap();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let id = library.tracks(&all_kinds()).unwrap()[0].id;
    (library, dir, id)
}

#[test]
fn embedded_cover_is_returned() {
    let (library, _dir) = scanned_library();
    let id = track_titled(&library, "Come Together").id;

    let artwork = library.artwork(id).unwrap().unwrap();

    assert_eq!(artwork.mime, "image/png");
    assert!(artwork.bytes.starts_with(PNG_SIGNATURE));
}

#[test]
fn tracks_without_art_return_none() {
    let (library, _dir) = scanned_library();

    assert!(
        library
            .artwork(track_titled(&library, "Zombie").id)
            .unwrap()
            .is_none()
    );
    assert!(library.artwork(i64::MAX).unwrap().is_none());
}

#[test]
fn embedded_art_beats_folder_art() {
    let (library, dir, id) = single_track_library("come_together.flac");
    fs::write(dir.path().join("cover.jpg"), b"folder").unwrap();

    let artwork = library.artwork(id).unwrap().unwrap();

    assert_eq!(artwork.mime, "image/png");
}

#[test]
fn folder_art_names_match_case_insensitively() {
    let (library, dir, id) = single_track_library("halo.mp3");
    fs::write(dir.path().join("Cover.JPG"), b"jpeg bytes").unwrap();

    let artwork = library.artwork(id).unwrap().unwrap();

    assert_eq!(artwork.mime, "image/jpeg");
    assert_eq!(artwork.bytes, b"jpeg bytes");
}

#[test]
fn folder_art_follows_the_documented_priority() {
    let (library, dir, id) = single_track_library("halo.mp3");
    let names = [
        "I Am... Sasha Fierce.jpg",
        "front.png",
        "front.jpg",
        "folder.png",
        "folder.jpg",
        "cover.png",
        "cover.jpg",
    ];
    for name in names {
        fs::write(dir.path().join(name), name).unwrap();
    }

    // Remove the winner each round; the next name in priority order takes over.
    for name in names.iter().rev() {
        let artwork = library.artwork(id).unwrap().unwrap();
        assert_eq!(artwork.bytes, name.as_bytes(), "expected {name} to win");
        fs::remove_file(dir.path().join(name)).unwrap();
    }
    assert!(library.artwork(id).unwrap().is_none());
}

#[test]
fn album_named_art_uses_the_png_mime_for_png_files() {
    let (library, dir, id) = single_track_library("halo.mp3");
    fs::write(dir.path().join("i am... sasha fierce.PNG"), b"png bytes").unwrap();

    let artwork = library.artwork(id).unwrap().unwrap();

    assert_eq!(artwork.mime, "image/png");
}

#[test]
fn art_for_a_deleted_file_is_none_not_an_error() {
    let (library, dir, id) = single_track_library("come_together.flac");
    fs::remove_dir_all(dir.path()).unwrap();

    assert!(library.artwork(id).unwrap().is_none());
}
