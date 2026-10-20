use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use antistreaming_library::{Library, Track};
use image::{ImageFormat, Rgb, RgbImage};
use tempfile::TempDir;

use crate::common::{library_of, scanned_library, track, track_titled};

fn write_png(path: &Path, width: u32, height: u32, color: [u8; 3]) {
    RgbImage::from_pixel(width, height, Rgb(color))
        .save_with_format(path, ImageFormat::Png)
        .unwrap();
}

/// Tracks whose files exist (empty ones, which hold no embedded art) in
/// separate directories, all on one album unless told otherwise.
fn on_disk(tracks: &[(i64, &Path, &str)]) -> Library {
    let rows: Vec<Track> = tracks
        .iter()
        .map(|(id, dir, album)| {
            let path = dir.join(format!("track{id}.mp3"));
            fs::write(&path, b"no tags here").unwrap();
            Track {
                path: path.to_str().unwrap().to_owned(),
                ..track(*id, &format!("t{id}"), "Artist", album, 1_000)
            }
        })
        .collect();
    library_of(&rows)
}

fn dimensions(bytes: &[u8]) -> (u32, u32) {
    let image = image::load_from_memory(bytes).unwrap();
    (image.width(), image.height())
}

fn cached(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn thumbnails_fit_the_size_keep_the_aspect_ratio_and_are_never_enlarged() {
    let dir = TempDir::new().unwrap();
    write_png(&dir.path().join("cover.png"), 300, 200, [10, 200, 30]);
    let library = on_disk(&[(1, dir.path(), "Album")]);

    let small = library.artwork_thumbnail(1, 100).unwrap().unwrap();
    let (width, height) = dimensions(&small.bytes);
    assert_eq!(small.mime, "image/jpeg");
    assert_eq!(width, 100);
    assert!((66..=67).contains(&height), "{height}");
    assert!(small.bytes.starts_with(&[0xFF, 0xD8]), "a JPEG");
    let pixel = image::load_from_memory(&small.bytes).unwrap().to_rgb8();
    let Rgb([r, g, b]) = *pixel.get_pixel(50, 30);
    assert!(g > 150 && r < 80 && b < 80, "colours survive: {r} {g} {b}");

    let big = library.artwork_thumbnail(1, 500).unwrap().unwrap();
    assert_eq!(dimensions(&big.bytes), (300, 200));
    let tiny = library.artwork_thumbnail(1, 0).unwrap().unwrap();
    assert_eq!(
        dimensions(&tiny.bytes),
        (1, 1),
        "a zero size is read as the smallest"
    );
}

#[test]
fn embedded_art_is_thumbnailed_and_artwork_still_returns_the_original() {
    let (library, _dir) = scanned_library();
    let id = track_titled(&library, "Come Together").id;

    let thumbnail = library.artwork_thumbnail(id, 8).unwrap().unwrap();
    let original = library.artwork(id).unwrap().unwrap();

    assert_eq!(dimensions(&thumbnail.bytes), (8, 8));
    assert_eq!(original.mime, "image/png");
    assert_eq!(dimensions(&original.bytes), (16, 16));
    let Rgb([r, g, b]) = *image::load_from_memory(&thumbnail.bytes)
        .unwrap()
        .to_rgb8()
        .get_pixel(4, 4);
    assert!(
        r > 200 && g < 60 && b < 60,
        "the fixture cover is red: {r} {g} {b}"
    );
}

#[test]
fn thumbnails_are_cached_per_source_and_size_and_refresh_when_the_source_changes() {
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let cache_dir = cache.path().join("nested/thumbs");
    let cover = dir.path().join("cover.png");
    write_png(&cover, 200, 200, [200, 0, 0]);
    let mut library = on_disk(&[(1, dir.path(), "Album")]);
    library.set_artwork_cache(cache_dir.clone());

    let first = library.artwork_thumbnail(1, 64).unwrap().unwrap();
    assert_eq!(
        cached(&cache_dir).len(),
        1,
        "the directory is created on demand"
    );
    assert!(cached(&cache_dir)[0].ends_with("-64.jpg"));

    // Proof of a hit: the cached bytes come back, not a fresh render.
    let entry = cache_dir.join(&cached(&cache_dir)[0]);
    fs::write(&entry, b"sentinel").unwrap();
    assert_eq!(
        library.artwork_thumbnail(1, 64).unwrap().unwrap().bytes,
        b"sentinel"
    );

    // Another size is another entry.
    library.artwork_thumbnail(1, 32).unwrap().unwrap();
    assert_eq!(cached(&cache_dir).len(), 2);

    // A changed cover is a new key: the stale entry is ignored.
    write_png(&cover, 100, 100, [0, 0, 200]);
    let refreshed = library.artwork_thumbnail(1, 64).unwrap().unwrap();
    assert_ne!(refreshed.bytes, b"sentinel");
    assert_eq!(dimensions(&refreshed.bytes), (64, 64));
    assert_ne!(refreshed.bytes, first.bytes);
}

#[test]
fn a_touched_source_file_with_the_same_size_is_a_new_key() {
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    write_png(&dir.path().join("cover.png"), 50, 50, [1, 2, 3]);
    let mut library = on_disk(&[(1, dir.path(), "Album")]);
    library.set_artwork_cache(cache.path().to_path_buf());
    library.artwork_thumbnail(1, 16).unwrap().unwrap();
    let entry = cache.path().join(&cached(cache.path())[0]);
    fs::write(&entry, b"sentinel").unwrap();

    let file = fs::File::options()
        .write(true)
        .open(dir.path().join("cover.png"))
        .unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(600))
        .unwrap();

    assert_ne!(
        library.artwork_thumbnail(1, 16).unwrap().unwrap().bytes,
        b"sentinel"
    );
}

#[test]
fn a_track_with_no_art_is_remembered_and_picks_up_a_cover_that_appears() {
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let mut library = on_disk(&[(1, dir.path(), "Album")]);
    library.set_artwork_cache(cache.path().to_path_buf());

    assert!(library.artwork_thumbnail(1, 32).unwrap().is_none());
    let names = cached(cache.path());
    assert_eq!(names.len(), 1);
    assert!(names[0].ends_with(".none"));

    write_png(&dir.path().join("Folder.PNG"), 80, 80, [9, 9, 9]);
    assert!(
        library.artwork_thumbnail(1, 32).unwrap().is_some(),
        "the new cover changes the key"
    );
}

#[test]
fn tracks_borrow_art_from_the_rest_of_their_album() {
    let bare = TempDir::new().unwrap();
    let covered = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    write_png(&covered.path().join("cover.png"), 120, 120, [0, 90, 0]);
    write_png(&other.path().join("cover.png"), 120, 120, [90, 0, 0]);
    let cache = TempDir::new().unwrap();
    let mut library = on_disk(&[
        (1, bare.path(), "Album"),
        (2, covered.path(), "Album"),
        (3, bare.path(), "Nameless"),
        (4, other.path(), "Nameless"),
        (5, bare.path(), ""),
        (6, other.path(), ""),
    ]);
    library.set_artwork_cache(cache.path().to_path_buf());

    let borrowed = library.artwork_thumbnail(1, 32).unwrap().unwrap();
    let owner = library.artwork_thumbnail(2, 32).unwrap().unwrap();
    assert_eq!(
        borrowed.bytes, owner.bytes,
        "the same album, so the same cover"
    );
    assert!(
        library.artwork(1).unwrap().is_none(),
        "artwork() is per track"
    );

    // Own art beats a sibling track's.
    write_png(&bare.path().join("cover.png"), 120, 120, [0, 0, 90]);
    assert_ne!(
        library.artwork_thumbnail(1, 32).unwrap().unwrap().bytes,
        owner.bytes
    );

    // Songs of one artist with no album name are not one album.
    assert!(
        library.artwork_thumbnail(5, 32).unwrap().is_some(),
        "its own directory's cover"
    );
    fs::remove_file(bare.path().join("cover.png")).unwrap();
    assert!(library.artwork_thumbnail(5, 32).unwrap().is_none());
    assert!(library.artwork_thumbnail(i64::MAX, 32).unwrap().is_none());
}

#[test]
fn unreadable_pictures_and_missing_files_are_no_art_not_errors() {
    let dir = TempDir::new().unwrap();
    let broken_dir = TempDir::new().unwrap();
    fs::write(broken_dir.path().join("cover.jpg"), b"this is not a jpeg").unwrap();
    write_png(&dir.path().join("cover.png"), 40, 40, [5, 5, 5]);
    let library = on_disk(&[(1, broken_dir.path(), "Album"), (2, dir.path(), "Album")]);

    assert!(
        library.artwork_thumbnail(1, 16).unwrap().is_some(),
        "falls back to the album's good cover"
    );

    let lonely = on_disk(&[(1, broken_dir.path(), "Solo")]);
    assert!(lonely.artwork_thumbnail(1, 16).unwrap().is_none());
    fs::remove_file(broken_dir.path().join("track1.mp3")).unwrap();
    assert!(lonely.artwork_thumbnail(1, 16).unwrap().is_none());
}

#[test]
fn thumbnails_work_without_a_cache() {
    let dir = TempDir::new().unwrap();
    write_png(&dir.path().join("cover.png"), 90, 90, [5, 5, 5]);
    let library = on_disk(&[(1, dir.path(), "Album")]);

    let first = library.artwork_thumbnail(1, 30).unwrap().unwrap();
    let second = library.artwork_thumbnail(1, 30).unwrap().unwrap();

    assert_eq!(first, second, "rendering is deterministic");
}

/// Makes the file unreadable, as a cloud placeholder is until it is downloaded. Windows has no
/// such permission, so there the dataless check alone keeps the file closed.
#[cfg(unix)]
fn make_unreadable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
}

#[cfg(not(unix))]
fn make_unreadable(_path: &Path) {}

#[test]
fn a_cloud_only_song_has_no_art_and_is_not_cached_as_bare() {
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    // The image is here, but the song beside it is not: nothing is read for it.
    write_png(&dir.path().join("cover.png"), 40, 40, [1, 2, 3]);
    let mut library = on_disk(&[(1, dir.path(), "Album")]);
    make_unreadable(&dir.path().join("track1.mp3"));
    library.set_artwork_cache(cache.path().to_path_buf());
    library.set_dataless_check(|path| path.extension().is_some_and(|e| e == "mp3"));

    assert!(library.artwork(1).unwrap().is_none());
    assert!(library.artwork_thumbnail(1, 100).unwrap().is_none());
    assert!(cached(cache.path()).is_empty());
}

#[test]
fn the_album_fallback_never_reads_a_cloud_only_song() {
    let local = TempDir::new().unwrap();
    let flagged = TempDir::new().unwrap();
    let evicted = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    // Songs 2 and 3 are on the same album, each with a cover beside it that reading would
    // download: 2 is known to be in the cloud, 3 was evicted since the last scan.
    write_png(&flagged.path().join("cover.png"), 40, 40, [1, 2, 3]);
    write_png(&evicted.path().join("cover.png"), 40, 40, [4, 5, 6]);
    let mut library = on_disk(&[
        (1, local.path(), "Album"),
        (2, flagged.path(), "Album"),
        (3, evicted.path(), "Album"),
    ]);
    let second = library.track(2).unwrap().unwrap();
    library
        .import_tracks(&[Track {
            in_cloud: true,
            ..second
        }])
        .unwrap();
    make_unreadable(&flagged.path().join("track2.mp3"));
    make_unreadable(&evicted.path().join("track3.mp3"));
    library.set_artwork_cache(cache.path().to_path_buf());
    let evicted_dir = evicted.path().to_path_buf();
    library.set_dataless_check(move |path| path.starts_with(&evicted_dir));

    assert!(library.artwork_thumbnail(1, 100).unwrap().is_none());
    // Only song 1's own lookup is remembered; the others weren't looked at.
    assert_eq!(cached(cache.path()).len(), 1);
    assert!(cached(cache.path())[0].ends_with(".none"));
}

#[test]
fn a_cover_image_that_is_only_in_the_cloud_is_passed_over() {
    let dir = TempDir::new().unwrap();
    // `cover.png` is tried first, but it is a placeholder; the album-named image is local.
    write_png(&dir.path().join("cover.png"), 40, 40, [1, 2, 3]);
    write_png(&dir.path().join("album.png"), 40, 40, [4, 5, 6]);
    let mut library = on_disk(&[(1, dir.path(), "Album")]);
    library.set_dataless_check(|path| path.file_name().is_some_and(|name| name == "cover.png"));

    let art = library.artwork(1).unwrap().unwrap();

    assert_eq!(art.bytes, fs::read(dir.path().join("album.png")).unwrap());
}
