use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use antistreaming_library::{
    Library, MediaKind, ScanSummary, Track, TrackEdits, TrackOverrides, ensure_local, is_dataless,
};
use rusqlite::Connection;
use tempfile::TempDir;

use crate::common::{
    added_ids, all_kinds, copy_fixtures, fixtures_dir, rewind_schema, scan, track, track_titled,
};

/// Stands in for iCloud Drive: a file counts as a cloud placeholder while a
/// marker named like it, but with an `.evicted` extension, sits beside it.
fn cloud_library() -> Library {
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());
    library
}

fn evict(path: &Path) {
    fs::write(path.with_extension("evicted"), b"").unwrap();
}

fn download(path: &Path) {
    fs::remove_file(path.with_extension("evicted")).unwrap();
}

fn rescan(library: &mut Library, dir: &Path) -> ScanSummary {
    library.add_paths(&[dir.to_path_buf()], |_| {}).unwrap()
}

fn put(root: &Path, relative: &str, contents: &[u8]) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

/// A real fixture, copied to `relative` under the root.
fn put_fixture(root: &Path, relative: &str, fixture: &str) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::copy(fixtures_dir().join(fixture), &path).unwrap();
    path
}

#[test]
fn placeholders_are_described_by_their_names_and_never_opened() {
    let dir = TempDir::new().unwrap();
    // Contents no tag parser would accept: reading any of them would fail the scan.
    let contents = b"only a placeholder";
    for relative in [
        "Beyonc\u{e9}/I Am/03 - Halo.mp3",
        "Queen/Opera/1-02 Bohemian.m4a",
        "Loose Song.flac",
        "Austen/Emma/Chapter 1.m4b",
    ] {
        evict(&put(dir.path(), relative, contents));
    }
    let mut library = cloud_library();

    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!((summary.scanned, summary.added), (4, 4));
    assert!(summary.failed.is_empty(), "{:?}", summary.failed);
    let tracks = library.tracks(&all_kinds()).unwrap();
    assert!(tracks.iter().all(|t| t.in_cloud && t.duration_ms == 0));
    assert!(tracks.iter().all(|t| t.file_size == contents.len() as u64));
    let by_title = |title: &str| tracks.iter().find(|t| t.title == title).unwrap();

    let halo = by_title("Halo");
    assert_eq!(
        (halo.artist.as_str(), halo.album.as_str()),
        ("Beyonc\u{e9}", "I Am")
    );
    assert_eq!(
        (halo.track_number, halo.disc_number, halo.codec.as_str()),
        (Some(3), None, "MP3")
    );
    assert_eq!((halo.genre.as_str(), halo.year), ("", None));
    let bohemian = by_title("Bohemian");
    assert_eq!(
        (bohemian.disc_number, bohemian.track_number),
        (Some(1), Some(2))
    );
    assert_eq!(
        (bohemian.artist.as_str(), bohemian.album.as_str()),
        ("Queen", "Opera")
    );
    let loose = by_title("Loose Song");
    assert_eq!(
        (
            loose.artist.as_str(),
            loose.album.as_str(),
            loose.codec.as_str()
        ),
        ("", "", "FLAC")
    );
    let chapter = by_title("Chapter 1");
    assert_eq!(chapter.kind, MediaKind::Audiobook);
    // The placeholders already file under artists and albums.
    let albums: Vec<String> = library
        .albums(&all_kinds())
        .unwrap()
        .into_iter()
        .map(|a| a.album)
        .collect();
    assert!(albums.contains(&"I Am".to_owned()) && albums.contains(&"Opera".to_owned()));
}

#[test]
fn unchanged_placeholders_are_left_alone_on_rescan() {
    let dir = TempDir::new().unwrap();
    evict(&put(dir.path(), "A/B/01 One.mp3", b"x"));
    evict(&put(dir.path(), "A/B/02 Two.mp3", b"x"));
    let mut library = cloud_library();
    library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(
        (summary.unchanged, summary.updated, summary.added),
        (2, 0, 0)
    );
    assert!(
        library
            .tracks(&all_kinds())
            .unwrap()
            .iter()
            .all(|t| t.in_cloud)
    );
}

#[test]
fn a_placeholder_that_has_been_downloaded_is_read_properly_on_the_next_scan() {
    let dir = TempDir::new().unwrap();
    let path = put_fixture(
        dir.path(),
        "Beyonc\u{e9}/I Am/03 - Track Three.mp3",
        "halo.mp3",
    );
    evict(&path);
    let mut library = cloud_library();
    library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    let placeholder = library.tracks(&all_kinds()).unwrap().remove(0);
    assert_eq!(placeholder.title, "Track Three");
    assert!(placeholder.in_cloud && placeholder.genre.is_empty());
    library.record_play(placeholder.id, 77).unwrap();

    // The size and modification time do not change when the contents arrive.
    download(&path);
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!((summary.updated, summary.unchanged), (1, 0));
    let local = library.track(placeholder.id).unwrap().unwrap();
    assert_eq!(
        (local.title.as_str(), local.genre.as_str(), local.year),
        ("Halo", "Pop", Some(2008))
    );
    assert!(!local.in_cloud && local.duration_ms > 0);
    assert_eq!(local.codec, "MP3");
    assert_eq!(
        (local.play_count, local.date_added),
        (1, placeholder.date_added)
    );
    assert_eq!(rescan(&mut library, dir.path()).unchanged, 1);
}

#[test]
fn a_file_the_system_evicts_keeps_its_tags_and_is_flagged() {
    let dir = TempDir::new().unwrap();
    let path = put_fixture(dir.path(), "halo.mp3", "halo.mp3");
    let mut library = cloud_library();
    rescan(&mut library, dir.path());
    let before = library.tracks(&all_kinds()).unwrap().remove(0);
    assert!(!before.in_cloud);

    evict(&path);
    let summary = rescan(&mut library, dir.path());

    assert_eq!((summary.unchanged, summary.updated), (1, 0));
    let evicted = library.track(before.id).unwrap().unwrap();
    assert!(evicted.in_cloud);
    assert_eq!(evicted.title, "Halo", "the tags it had stay");
    assert_eq!(evicted.duration_ms, before.duration_ms);

    download(&path);
    assert_eq!(rescan(&mut library, dir.path()).unchanged, 1);
    assert!(!library.track(before.id).unwrap().unwrap().in_cloud);
}

#[test]
fn a_file_that_changes_while_evicted_goes_by_its_name_until_it_is_downloaded() {
    let dir = TempDir::new().unwrap();
    let path = put_fixture(dir.path(), "05 - Remix.mp3", "halo.mp3");
    let mut library = cloud_library();
    rescan(&mut library, dir.path());
    let id = library.tracks(&all_kinds()).unwrap()[0].id;
    assert_eq!(library.track(id).unwrap().unwrap().title, "Halo");

    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(600))
        .unwrap();
    evict(&path);
    let summary = rescan(&mut library, dir.path());

    assert_eq!(summary.updated, 1);
    let placeholder = library.track(id).unwrap().unwrap();
    assert_eq!(
        (
            placeholder.title.as_str(),
            placeholder.track_number,
            placeholder.in_cloud
        ),
        ("Remix", Some(5), true)
    );
    download(&path);
    rescan(&mut library, dir.path());
    let local = library.track(id).unwrap().unwrap();
    assert_eq!((local.title.as_str(), local.in_cloud), ("Halo", false));
}

#[test]
fn refreshing_re_reads_the_files_that_became_local_and_reports_only_those() {
    let dir = TempDir::new().unwrap();
    let halo = put_fixture(dir.path(), "Beyonc\u{e9}/I Am/03 - A.mp3", "halo.mp3");
    let zombie = put_fixture(dir.path(), "Cranberries/Argue/02 - B.ogg", "zombie.ogg");
    let something = put_fixture(dir.path(), "Beatles/Road/02 - C.m4a", "something.m4a");
    let local = put_fixture(dir.path(), "come_together.flac", "come_together.flac");
    for placeholder in [&halo, &zombie, &something] {
        evict(placeholder);
    }
    let mut library = cloud_library();
    rescan(&mut library, dir.path());
    let ids: Vec<i64> = library
        .tracks(&all_kinds())
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    let id_of = |title: &str| {
        library
            .tracks(&all_kinds())
            .unwrap()
            .into_iter()
            .find(|t| t.title == title)
            .unwrap()
            .id
    };
    let (halo_id, zombie_id, something_id, local_id) =
        (id_of("A"), id_of("B"), id_of("C"), id_of("Come Together"));

    assert!(
        library.refresh_cloud_status(&ids).unwrap().is_empty(),
        "nothing has changed yet"
    );

    download(&halo);
    download(&zombie);
    let refreshed = library
        .refresh_cloud_status(&[something_id, zombie_id, 9_999, halo_id, local_id])
        .unwrap();

    assert_eq!(
        refreshed,
        [zombie_id, halo_id],
        "in the order asked, only the ones that changed"
    );
    let halo_row = library.track(halo_id).unwrap().unwrap();
    assert_eq!(
        (
            halo_row.title.as_str(),
            halo_row.genre.as_str(),
            halo_row.in_cloud
        ),
        ("Halo", "Pop", false)
    );
    assert_eq!(library.track(zombie_id).unwrap().unwrap().title, "Zombie");
    assert!(
        library.track(something_id).unwrap().unwrap().in_cloud,
        "still a placeholder"
    );

    // The system takes a local file back.
    evict(&local);
    assert_eq!(
        library.refresh_cloud_status(&[local_id, halo_id]).unwrap(),
        [local_id]
    );
    let evicted = library.track(local_id).unwrap().unwrap();
    assert!(evicted.in_cloud);
    assert_eq!(evicted.title, "Come Together");
    assert!(library.refresh_cloud_status(&[]).unwrap().is_empty());
}

#[test]
fn refreshing_re_applies_overrides_and_skips_files_it_cannot_read_or_find() {
    let dir = TempDir::new().unwrap();
    let pinned = put_fixture(dir.path(), "pinned.mp3", "halo.mp3");
    let broken = put(dir.path(), "broken.mp3", b"not audio at all");
    let gone = put(dir.path(), "gone.mp3", b"x");
    evict(&broken);
    evict(&gone);
    let mut library = cloud_library();
    let overrides = TrackOverrides {
        title: Some("Pinned".into()),
        ..TrackOverrides::default()
    };
    let pinned_id = added_ids(&mut library, &[(pinned.clone(), overrides)])[0];
    rescan(&mut library, dir.path());
    let id_of = |library: &Library, title: &str| {
        library
            .tracks(&all_kinds())
            .unwrap()
            .into_iter()
            .find(|t| t.title == title)
            .unwrap()
            .id
    };
    let broken_id = id_of(&library, "broken");
    let gone_id = id_of(&library, "gone");

    evict(&pinned);
    assert_eq!(
        library.refresh_cloud_status(&[pinned_id]).unwrap(),
        [pinned_id]
    );
    download(&pinned);
    assert_eq!(
        library.refresh_cloud_status(&[pinned_id]).unwrap(),
        [pinned_id]
    );
    assert_eq!(library.track(pinned_id).unwrap().unwrap().title, "Pinned");

    download(&broken);
    fs::remove_file(&gone).unwrap();
    let refreshed = library.refresh_cloud_status(&[broken_id, gone_id]).unwrap();
    assert!(refreshed.is_empty());
    assert!(
        library.track(broken_id).unwrap().unwrap().in_cloud,
        "left flagged for another try"
    );
}

#[test]
fn the_cloud_flag_survives_reopening_json_and_import() {
    let dir = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let db_path = db.path().join("library.db");
    let path = put(dir.path(), "A/B/01 Song.mp3", b"x");
    evict(&path);
    {
        let mut library = Library::open(&db_path).unwrap();
        library.set_dataless_check(|path| path.with_extension("evicted").exists());
        rescan(&mut library, dir.path());
    }

    let library = Library::open(&db_path).unwrap();
    let tracks = library.tracks(&all_kinds()).unwrap();
    assert!(tracks[0].in_cloud);
    assert_eq!(serde_json::to_value(&tracks[0]).unwrap()["inCloud"], true);

    // Placeholders are stamped as never parsed so that a download is noticed.
    let parser_versions: Vec<u32> = Connection::open(&db_path)
        .unwrap()
        .prepare("SELECT parser_version FROM tracks")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(parser_versions, [0]);

    let mut imported = Library::open_in_memory().unwrap();
    let cloud_row = Track {
        in_cloud: true,
        ..track(1, "x", "y", "z", 1)
    };
    imported
        .import_tracks(&[cloud_row, track(2, "local", "y", "z", 1)])
        .unwrap();
    assert!(imported.track(1).unwrap().unwrap().in_cloud);
    assert!(!imported.track(2).unwrap().unwrap().in_cloud);
}

#[test]
fn a_library_from_before_cloud_tracking_upgrades_with_local_files() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let fixtures = copy_fixtures();
    {
        let mut library = Library::open(&db_path).unwrap();
        scan(&mut library, fixtures.path());
    }
    rewind_schema(&db_path, 4);

    let mut library = Library::open(&db_path).unwrap();

    assert!(
        library
            .tracks(&all_kinds())
            .unwrap()
            .iter()
            .all(|t| !t.in_cloud)
    );
    let rescan = library
        .add_paths(&[fixtures.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!(
        rescan.unchanged, 8,
        "existing rows were parsed, so nothing is re-read"
    );
    assert_eq!(track_titled(&library, "Halo").genre, "Pop");
}

#[test]
fn ensure_local_returns_at_once_for_files_that_are_already_here() {
    let dir = TempDir::new().unwrap();
    let path = put(dir.path(), "song.mp3", b"present");
    let mut reported = false;

    ensure_local(&path, &mut |_, _| reported = true, &AtomicBool::new(false)).unwrap();
    ensure_local(&path, &mut |_, _| reported = true, &AtomicBool::new(true)).unwrap();
    ensure_local(
        &dir.path().join("missing.mp3"),
        &mut |_, _| reported = true,
        &AtomicBool::new(false),
    )
    .unwrap();

    assert!(!reported);
}

#[test]
fn ordinary_and_missing_files_are_not_placeholders() {
    let dir = TempDir::new().unwrap();
    let path = put(dir.path(), "song.mp3", b"present");

    assert!(!is_dataless(&path));
    assert!(!is_dataless(dir.path()));
    assert!(!is_dataless(&dir.path().join("missing.mp3")));
}

/// What Music knows about a song and a placeholder cannot say.
fn from_music(title: &str, number: u32) -> TrackOverrides {
    TrackOverrides {
        title: Some(title.into()),
        artist: Some("Music Artist".into()),
        album: Some("Music Album".into()),
        album_artist: Some("Music Album Artist".into()),
        genre: Some("Music Genre".into()),
        composer: Some("Music Composer".into()),
        year: Some(1999),
        track_number: Some(number),
        track_count: Some(12),
        disc_number: Some(1),
        disc_count: Some(2),
        compilation: Some(true),
        date_added: Some(1_500_000_000),
        ..TrackOverrides::default()
    }
}

#[test]
fn adding_files_makes_placeholder_rows_without_opening_the_files() {
    let dir = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let db_path = db.path().join("library.db");
    // None of these could be parsed, and one cannot even be opened.
    let first = put(dir.path(), "Artist/Album/01 First.mp3", b"not audio");
    let second = put(dir.path(), "Artist/Album/02 Second.m4a", b"not audio");
    let third = put(dir.path(), "Artist/Album/03 Third.flac", b"not audio");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&third, fs::Permissions::from_mode(0o000)).unwrap();
    }
    for file in [&first, &second, &third] {
        evict(file);
    }
    let mut library = Library::open(&db_path).unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());

    let summary = library
        .add_files(&[
            (first.clone(), from_music("One", 1)),
            (second.clone(), from_music("Two", 2)),
            (third.clone(), TrackOverrides::default()),
        ])
        .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&third, fs::Permissions::from_mode(0o644)).unwrap();
    }
    assert!(summary.failed.is_empty(), "{:?}", summary.failed);
    let ids: Vec<i64> = summary.ids.iter().map(|id| id.unwrap()).collect();
    let rows: Vec<Track> = ids
        .iter()
        .map(|id| library.track(*id).unwrap().unwrap())
        .collect();
    assert!(rows.iter().all(|row| row.in_cloud && row.duration_ms == 0));
    let one = &rows[0];
    assert_eq!(
        (one.title.as_str(), one.artist.as_str(), one.album.as_str()),
        ("One", "Music Artist", "Music Album")
    );
    assert_eq!(
        (
            one.album_artist.as_str(),
            one.genre.as_str(),
            one.composer.as_str()
        ),
        ("Music Album Artist", "Music Genre", "Music Composer")
    );
    assert_eq!(
        (
            one.year,
            one.track_number,
            one.track_count,
            one.disc_number,
            one.disc_count,
            one.compilation,
            one.date_added
        ),
        (
            Some(1999),
            Some(1),
            Some(12),
            Some(1),
            Some(2),
            true,
            1_500_000_000
        )
    );
    assert_eq!((one.codec.as_str(), rows[1].codec.as_str()), ("MP3", "AAC"));
    // Without overrides it is described by its name, as a scan would.
    let bare = &rows[2];
    assert_eq!(
        (bare.title.as_str(), bare.track_number, bare.codec.as_str()),
        ("Third", Some(3), "FLAC")
    );
    let versions: Vec<u32> = Connection::open(&db_path)
        .unwrap()
        .prepare("SELECT parser_version FROM tracks")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(versions, [0, 0, 0], "each is read once it is local");

    // The first is downloaded: the real file arrives, and the scan reads it
    // properly while what Music said stays.
    download(&first);
    fs::copy(fixtures_dir().join("halo.mp3"), &first).unwrap();
    rescan(&mut library, dir.path());
    let read = library.track(ids[0]).unwrap().unwrap();
    assert!(!read.in_cloud && read.duration_ms > 0);
    assert_eq!(
        (
            read.title.as_str(),
            read.composer.as_str(),
            read.track_number
        ),
        ("One", "Music Composer", Some(1))
    );
    assert!(read.compilation && read.disc_count == Some(2));
    assert!(library.track(ids[1]).unwrap().unwrap().in_cloud);
}

#[test]
fn adding_a_known_file_that_is_now_a_placeholder_keeps_what_was_read_from_it() {
    let dir = TempDir::new().unwrap();
    let path = put_fixture(dir.path(), "halo.mp3", "halo.mp3");
    let mut library = cloud_library();
    rescan(&mut library, dir.path());
    let before = library.tracks(&all_kinds()).unwrap().remove(0);
    assert!(!before.in_cloud && before.duration_ms > 0);
    evict(&path);

    let summary = library
        .add_files(&[(path.clone(), from_music("Renamed", 4))])
        .unwrap();

    assert_eq!(summary.ids, [Some(before.id)]);
    let after = library.track(before.id).unwrap().unwrap();
    assert!(after.in_cloud);
    assert_eq!(
        (
            after.title.as_str(),
            after.composer.as_str(),
            after.track_number
        ),
        ("Renamed", "Music Composer", Some(4))
    );
    assert_eq!(
        (after.duration_ms, after.bitrate_kbps, after.codec.as_str()),
        (
            before.duration_ms,
            before.bitrate_kbps,
            before.codec.as_str()
        ),
        "the details read before it was evicted stand"
    );
    // Downloaded again, it is read and the overrides still hold.
    download(&path);
    rescan(&mut library, dir.path());
    let reread = library.track(before.id).unwrap().unwrap();
    assert!(!reread.in_cloud);
    assert_eq!(
        (reread.title.as_str(), reread.disc_count),
        ("Renamed", Some(2))
    );
}

#[test]
fn placeholders_are_not_copied_into_the_media_folder_and_duplicates_are_one_row() {
    let dir = TempDir::new().unwrap();
    let media = TempDir::new().unwrap();
    let path = put(dir.path(), "A/B/01 Song.mp3", b"not audio");
    evict(&path);
    let mut library = cloud_library();
    library.set_media_folder(media.path()).unwrap();
    library.set_copy_on_add(true).unwrap();
    library.set_organize(true).unwrap();

    let summary = library
        .add_files(&[
            (path.clone(), from_music("First", 1)),
            (path.clone(), from_music("Second", 1)),
        ])
        .unwrap();

    assert!(summary.failed.is_empty());
    assert_eq!(summary.ids[0], summary.ids[1]);
    assert_eq!(library.stats(&all_kinds()).unwrap().count, 1);
    let row = library.track(summary.ids[0].unwrap()).unwrap().unwrap();
    assert_eq!(row.title, "Second", "the later one wins");
    assert!(Path::new(&row.path).ends_with("A/B/01 Song.mp3"));
    assert_eq!(fs::read_dir(media.path()).unwrap().count(), 0);
}

#[test]
fn adding_in_place_again_keeps_the_users_edits_and_only_fills_what_is_missing() {
    let dir = TempDir::new().unwrap();
    let path = put(dir.path(), "Artist/Album/01 Song.m4a", b"not audio");
    evict(&path);
    let mut library = cloud_library();
    // Music has no genre or year for it yet.
    let first = TrackOverrides {
        genre: None,
        year: None,
        ..from_music("Music Title", 1)
    };
    let id = library
        .add_files_in_place(&[(path.clone(), first)])
        .unwrap()
        .ids[0]
        .unwrap();
    library
        .edit_tracks(
            &[id],
            &TrackEdits {
                title: Some("My Title".into()),
                lyrics: Some("la la la".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    // Since then Music has retitled it and learned its genre, year and length.
    let again = TrackOverrides {
        duration_ms: Some(123_000),
        ..from_music("Retitled in Music", 1)
    };
    library
        .add_files_in_place(&[(path.clone(), again)])
        .unwrap();

    let row = library.track(id).unwrap().unwrap();
    assert_eq!(
        (row.title.as_str(), row.genre.as_str(), row.year),
        ("My Title", "Music Genre", Some(1999))
    );
    assert_eq!((row.has_lyrics, row.duration_ms), (true, 123_000));
    assert_eq!(library.lyrics(id).unwrap().unwrap().text, "la la la");
    assert_eq!(library.stats(&all_kinds()).unwrap().count, 1);
}

#[test]
fn a_scanned_placeholder_takes_musics_details_on_import_but_keeps_an_edit() {
    let dir = TempDir::new().unwrap();
    let plain = put(dir.path(), "Folder/Guess/01 Plain.m4a", b"not audio");
    let edited = put(dir.path(), "Folder/Guess/02 Edited.m4a", b"not audio");
    evict(&plain);
    evict(&edited);
    let mut library = cloud_library();
    // Added the ordinary way, its details are guesses from the path.
    rescan(&mut library, dir.path());
    let (plain_id, edited_id) = (
        track_titled(&library, "Plain").id,
        track_titled(&library, "Edited").id,
    );
    library
        .edit_tracks(
            &[edited_id],
            &TrackEdits {
                title: Some("My Edit".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    library
        .add_files_in_place(&[
            (plain, from_music("Music Plain", 7)),
            (edited, from_music("Music Edited", 8)),
        ])
        .unwrap();

    let plain = library.track(plain_id).unwrap().unwrap();
    assert_eq!(
        (
            plain.title.as_str(),
            plain.artist.as_str(),
            plain.album.as_str(),
            plain.track_number
        ),
        ("Music Plain", "Music Artist", "Music Album", Some(7))
    );
    let edited = library.track(edited_id).unwrap().unwrap();
    assert_eq!(
        (
            edited.title.as_str(),
            edited.artist.as_str(),
            edited.track_number
        ),
        ("My Edit", "Music Artist", Some(8))
    );
}

#[test]
fn editing_a_file_evicted_since_the_last_scan_changes_only_the_library() {
    let dir = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let db_path = db.path().join("library.db");
    let path = put_fixture(dir.path(), "dump/halo.mp3", "halo.mp3");
    let original = fs::read(&path).unwrap();
    let mut library = Library::open(&db_path).unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());
    rescan(&mut library, dir.path());
    let id = track_titled(&library, "Halo").id;
    assert!(!library.track(id).unwrap().unwrap().in_cloud);
    // Keeping the folder organized would move it for the new title and album.
    library.set_media_folder(dir.path()).unwrap();
    library.set_organize(true).unwrap();
    evict(&path);

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                title: Some("Halo (Edit)".into()),
                album: Some("Fierce".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert!(report.written.is_empty() && report.moved.is_empty());
    assert_eq!(report.db_only.len(), 1);
    assert_eq!(
        fs::read(&path).unwrap(),
        original,
        "the file is not rewritten"
    );
    let row = library.track(id).unwrap().unwrap();
    assert_eq!(
        (row.title.as_str(), row.album.as_str()),
        ("Halo (Edit)", "Fierce")
    );
    assert_eq!(row.path, fs::canonicalize(&path).unwrap().to_str().unwrap());
    let pinned: String = Connection::open(&db_path)
        .unwrap()
        .query_row("SELECT overrides FROM tracks WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(pinned.contains("Halo (Edit)"), "{pinned}");
}

#[test]
fn a_placeholder_takes_its_length_from_music_but_a_file_keeps_its_own() {
    let dir = TempDir::new().unwrap();
    let cloud = put(dir.path(), "A/B/01 Cloud.m4a", b"not audio");
    evict(&cloud);
    let local = put_fixture(dir.path(), "A/B/02 Local.mp3", "halo.mp3");
    let mut library = cloud_library();
    let music_says = TrackOverrides {
        duration_ms: Some(1),
        ..TrackOverrides::default()
    };

    let summary = library
        .add_files_in_place(&[(cloud, music_says.clone()), (local, music_says)])
        .unwrap();

    let [cloud, local] = [0, 1].map(|n| library.track(summary.ids[n].unwrap()).unwrap().unwrap());
    assert_eq!(cloud.duration_ms, 1);
    assert!(local.duration_ms > 1, "{}", local.duration_ms);
}

#[test]
#[ignore = "timing check; needs --release"]
fn forty_thousand_placeholders_are_added_in_seconds() {
    let dir = TempDir::new().unwrap();
    let mut files = Vec::new();
    for artist in 0..2_000 {
        let folder = dir
            .path()
            .join(format!("Artist {artist}/Album {}", artist % 7));
        fs::create_dir_all(&folder).unwrap();
        for song in 0..20 {
            let path = folder.join(format!("{:02} Song {artist}-{song}.m4a", song + 1));
            fs::write(&path, b"").unwrap();
            files.push((path, from_music(&format!("Song {artist}-{song}"), song + 1)));
        }
    }
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|_| true);

    let started = std::time::Instant::now();
    let summary = library.add_files(&files).unwrap();
    let added = started.elapsed();
    let again = std::time::Instant::now();
    let summary_again = library.add_files(&files).unwrap();
    let updated = again.elapsed();

    eprintln!("40,000 placeholders: added in {added:?}, added again in {updated:?}");
    assert!(summary.failed.is_empty() && summary_again.failed.is_empty());
    assert_eq!(summary.ids, summary_again.ids);
    assert!(summary.ids.iter().all(Option::is_some));
    assert_eq!(library.stats(&all_kinds()).unwrap().count, 40_000);
    assert!(added.as_secs() < 10 && updated.as_secs() < 10);
}
