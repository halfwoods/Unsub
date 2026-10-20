//! `locate_in_folder` over folders on the real disk.

use std::fs;
use std::path::Path;

use antistreaming_applemusic::{
    Confidence, Error, LocateStage, MediaType, MusicLibraryItem, Ownership, locate_in_folder,
};

fn song(id: &str, title: &str, artist: &str, album: &str, track: u32) -> MusicLibraryItem {
    MusicLibraryItem {
        persistent_id: id.to_owned(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album_artist: String::new(),
        album: album.to_owned(),
        genre: String::new(),
        duration_ms: Some(180_000),
        track_number: Some(track),
        disc_number: None,
        year: None,
        kind: "MPEG audio file".to_owned(),
        media: MediaType::Music,
        ownership: Ownership::Unknown,
        location: None,
        file_present: false,
        play_count: 0,
        skip_count: 0,
        rating: 0,
        date_added: None,
        last_played: None,
        last_skipped: None,
        enabled: true,
    }
}

fn write(root: &Path, path: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, b"audio").unwrap();
}

#[test]
fn finds_files_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "Radiohead/OK Computer/01 Airbag.mp3");
    write(dir.path(), "Radiohead/OK Computer/cover.jpg");
    let items = [
        song("a", "Airbag", "Radiohead", "OK Computer", 1),
        song("b", "Lucky", "Radiohead", "OK Computer", 3),
    ];

    let located = locate_in_folder(&items, dir.path(), &mut |_| {}).unwrap();

    let found = &located.matched["a"];
    assert_eq!(
        found.path,
        dir.path().join("Radiohead/OK Computer/01 Airbag.mp3")
    );
    assert_eq!(found.confidence, Confidence::Exact);
    assert!(!found.dataless, "a file written normally has its content");
    assert_eq!(located.unmatched, ["b"]);
}

/// Files nobody may read: finding them must not need to.
#[cfg(unix)]
#[test]
fn never_needs_to_read_a_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    for path in ["A/B/01 One.mp3", "A/B/mystery name.mp3"] {
        write(dir.path(), path);
        fs::set_permissions(dir.path().join(path), fs::Permissions::from_mode(0o000)).unwrap();
    }
    let items = [
        song("a", "One", "A", "B", 1),
        song("b", "Unrelated", "A", "B", 2),
    ];

    let located = locate_in_folder(&items, dir.path(), &mut |_| {}).unwrap();

    // Both are found: one by its name, the other by being the right size for what is left.
    assert_eq!(located.matched.len(), 2, "{located:?}");
    assert!(
        File::open(dir.path().join("A/B/01 One.mp3")).is_err(),
        "the files really are unreadable"
    );
}

#[cfg(unix)]
use std::fs::File;

#[test]
fn folders_written_decomposed_are_found_and_reported_as_written() {
    let dir = tempfile::tempdir().unwrap();
    // "Björk" with the ö as o plus a combining diaeresis, as older volumes stored it.
    write(dir.path(), "Bjo\u{0308}rk/Homogenic/01 Hunter.mp3");
    let items = [song("a", "Hunter", "Bj\u{00F6}rk", "Homogenic", 1)];

    let located = locate_in_folder(&items, dir.path(), &mut |_| {}).unwrap();

    let found = &located.matched["a"];
    assert_eq!(found.confidence, Confidence::Exact);
    assert!(
        found.path.exists(),
        "the path is the one on disk: {:?}",
        found.path
    );
}

#[test]
fn looks_inside_a_media_folder_for_the_music() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "Music/A/B/01 Song.mp3");
    write(dir.path(), "Podcasts/Show/Episode.mp3");

    let located =
        locate_in_folder(&[song("a", "Song", "A", "B", 1)], dir.path(), &mut |_| {}).unwrap();

    assert_eq!(located.matched["a"].confidence, Confidence::Exact);
}

#[test]
fn a_folder_that_is_not_there_is_an_error() {
    let dir = tempfile::tempdir().unwrap();

    let result = locate_in_folder(&[], &dir.path().join("missing"), &mut |_| {});

    assert!(matches!(result, Err(Error::Io(_))));
}

#[test]
fn reports_progress() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "A/B/01 Song.mp3");
    let mut stages = Vec::new();

    locate_in_folder(
        &[song("a", "Song", "A", "B", 1)],
        dir.path(),
        &mut |progress| {
            if stages.last() != Some(&progress.stage) {
                stages.push(progress.stage);
            }
        },
    )
    .unwrap();

    assert_eq!(stages, [LocateStage::Scanning, LocateStage::Matching]);
}

#[test]
fn results_serialize_for_the_app() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "A/B/01 Song.mp3");
    let located =
        locate_in_folder(&[song("a", "Song", "A", "B", 1)], dir.path(), &mut |_| {}).unwrap();

    let json = serde_json::to_value(&located).unwrap();

    assert_eq!(json["matched"]["a"]["confidence"], "exact");
    assert_eq!(json["matched"]["a"]["dataless"], false);
    assert_eq!(json["unmatched"], serde_json::json!([]));
    let restored: antistreaming_applemusic::Located = serde_json::from_value(json).unwrap();
    assert_eq!(restored, located);
}
