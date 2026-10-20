use std::fs::{self, File};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use antistreaming_library::{Library, MediaKind, TrackOverrides, TrackQuery};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{Accessor, ItemKey};
use rusqlite::Connection;
use tempfile::TempDir;

use crate::common::{added_ids, all_kinds, copy_fixtures, fixtures_dir, scan, track_titled};

fn episode_overrides() -> TrackOverrides {
    TrackOverrides {
        kind: Some(MediaKind::Podcast),
        title: Some("Episode 7: The Heist".into()),
        artist: Some("Heist Weekly".into()),
        album: Some("Heist Weekly".into()),
        album_artist: Some("Heist Weekly".into()),
        genre: Some("Podcast".into()),
        date_added: Some(1_500_000_000),
        year: Some(2024),
        ..TrackOverrides::default()
    }
}

fn one_file() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("halo.mp3");
    fs::copy(fixtures_dir().join("halo.mp3"), &path).unwrap();
    (dir, path)
}

#[test]
fn overrides_win_over_the_tags() {
    let (_dir, path) = one_file();
    let mut library = Library::open_in_memory().unwrap();

    let ids = added_ids(&mut library, &[(path, episode_overrides())]);

    let track = library.track(ids[0]).unwrap().unwrap();
    assert_eq!(track.title, "Episode 7: The Heist");
    assert_eq!(
        (track.artist.as_str(), track.album.as_str()),
        ("Heist Weekly", "Heist Weekly")
    );
    assert_eq!(
        (track.album_artist.as_str(), track.genre.as_str()),
        ("Heist Weekly", "Podcast")
    );
    assert_eq!(
        (track.kind, track.year, track.date_added),
        (MediaKind::Podcast, Some(2024), 1_500_000_000)
    );
    assert_eq!(
        track.codec, "MP3",
        "everything not overridden still comes from the file"
    );
    assert!(track.duration_ms > 0);
    // Derived columns follow the overrides.
    let found = library
        .tracks(&TrackQuery {
            kind: None,
            search: Some("heist".into()),
            ..TrackQuery::default()
        })
        .unwrap();
    assert_eq!(found.len(), 1);
    assert!(
        library.tracks(&TrackQuery::default()).unwrap().is_empty(),
        "no longer music"
    );
    assert_eq!(
        library.albums(&all_kinds()).unwrap()[0].album,
        "Heist Weekly"
    );
}

#[test]
fn files_without_overrides_read_as_usual_and_ids_follow_input_order() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    let files = vec![
        (dir.path().join("zombie.ogg"), TrackOverrides::default()),
        (
            dir.path().join("halo.mp3"),
            TrackOverrides {
                genre: Some("Pop Rock".into()),
                ..TrackOverrides::default()
            },
        ),
        (dir.path().join("zombie.ogg"), TrackOverrides::default()),
    ];

    let ids = added_ids(&mut library, &files);

    assert_eq!(ids.len(), 3);
    assert_eq!(ids[0], ids[2], "the same file is the same track");
    assert_ne!(ids[0], ids[1]);
    let zombie = library.track(ids[0]).unwrap().unwrap();
    assert_eq!(
        (zombie.title.as_str(), zombie.artist.as_str()),
        ("Zombie", "The Cranberries")
    );
    let halo = library.track(ids[1]).unwrap().unwrap();
    assert_eq!(
        (halo.title.as_str(), halo.genre.as_str()),
        ("Halo", "Pop Rock")
    );
    assert_eq!(halo.kind, MediaKind::Music);
    assert_eq!(library.stats(&all_kinds()).unwrap().count, 2);
}

#[test]
fn overrides_survive_rescans_even_when_the_file_changes() {
    let (dir, path) = one_file();
    let db = TempDir::new().unwrap();
    let db_path = db.path().join("library.db");
    let mut library = Library::open(&db_path).unwrap();
    let overrides = TrackOverrides {
        title: Some("Pinned Title".into()),
        kind: Some(MediaKind::Podcast),
        ..TrackOverrides::default()
    };
    let id = added_ids(&mut library, &[(path.clone(), overrides)])[0];

    // Nothing changed: the scan leaves it alone.
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!(
        (summary.unchanged, summary.updated, summary.added),
        (1, 0, 0)
    );

    // The file's tags change, so it is re-read: the pinned fields hold, the rest follow the file.
    let mut tagged = lofty::read_from_path(&path).unwrap();
    let tag = tagged.primary_tag_mut().unwrap();
    tag.set_title("Retagged".into());
    tag.set_album("Retagged Album".into());
    tagged.save_to_path(&path, WriteOptions::default()).unwrap();
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!(summary.updated, 1);
    let track = library.track(id).unwrap().unwrap();
    assert_eq!(
        (track.title.as_str(), track.kind),
        ("Pinned Title", MediaKind::Podcast)
    );
    assert_eq!(track.album, "Retagged Album");

    // A newer tag parser re-reads everything too.
    Connection::open(&db_path)
        .unwrap()
        .execute("UPDATE tracks SET parser_version = 1", [])
        .unwrap();
    library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!(library.track(id).unwrap().unwrap().title, "Pinned Title");
}

#[test]
fn adding_a_known_file_applies_overrides_and_keeps_its_history() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let halo = track_titled(&library, "Halo");
    library.record_play(halo.id, 99).unwrap();
    library.set_rating(halo.id, 80).unwrap();
    let path = dir.path().join("halo.mp3");

    let ids = added_ids(
        &mut library,
        &[(
            path.clone(),
            TrackOverrides {
                title: Some("Renamed".into()),
                date_added: Some(1_234),
                ..TrackOverrides::default()
            },
        )],
    );

    assert_eq!(ids, [halo.id]);
    let track = library.track(halo.id).unwrap().unwrap();
    assert_eq!((track.title.as_str(), track.date_added), ("Renamed", 1_234));
    assert_eq!((track.play_count, track.rating), (1, 80));
    assert_eq!(library.stats(&all_kinds()).unwrap().count, 8);

    // Adding again with no overrides lifts them: the next re-read returns to the tags.
    added_ids(&mut library, &[(path.clone(), TrackOverrides::default())]);
    assert_eq!(library.track(halo.id).unwrap().unwrap().title, "Halo");
    let file = File::options().write(true).open(&path).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(300))
        .unwrap();
    library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!(library.track(halo.id).unwrap().unwrap().title, "Halo");
}

#[test]
fn a_bad_file_is_reported_and_the_rest_are_still_added() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    let broken = dir.path().join("broken.mp3");
    fs::write(&broken, "not audio").unwrap();
    let missing = dir.path().join("missing.mp3");

    let summary = library
        .add_files(&[
            (dir.path().join("halo.mp3"), TrackOverrides::default()),
            (missing.clone(), TrackOverrides::default()),
            (broken.clone(), TrackOverrides::default()),
            (dir.path().join("zombie.ogg"), TrackOverrides::default()),
        ])
        .unwrap();

    assert_eq!(summary.ids.len(), 4);
    assert!(summary.ids[0].is_some() && summary.ids[3].is_some());
    assert_eq!((summary.ids[1], summary.ids[2]), (None, None));
    let failed: Vec<&str> = summary
        .failed
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    assert_eq!(
        failed,
        [missing.to_str().unwrap(), broken.to_str().unwrap()]
    );
    assert!(summary.failed.iter().all(|(_, why)| !why.is_empty()));
    assert_eq!(library.stats(&all_kinds()).unwrap().count, 2);
    let empty = library.add_files(&[]).unwrap();
    assert!(empty.ids.is_empty() && empty.failed.is_empty());
}

#[test]
fn every_input_gets_the_id_of_its_track_new_or_already_known() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let known = track_titled(&library, "Halo").id;
    let count = library.stats(&all_kinds()).unwrap().count;
    let extra = dir.path().join("sub/copy.mp3");
    fs::create_dir_all(extra.parent().unwrap()).unwrap();
    fs::copy(fixtures_dir().join("halo.mp3"), &extra).unwrap();

    let summary = library
        .add_files(&[
            (
                dir.path().join("halo.mp3"),
                TrackOverrides {
                    title: Some("Renamed".into()),
                    ..TrackOverrides::default()
                },
            ),
            (extra, TrackOverrides::default()),
            (dir.path().join("./halo.mp3"), TrackOverrides::default()),
        ])
        .unwrap();

    assert!(summary.failed.is_empty());
    let [first, second, third] = summary.ids[..] else {
        panic!("one id per input: {:?}", summary.ids);
    };
    assert_eq!(first, Some(known), "a known path keeps its id");
    assert_eq!(third, Some(known), "the same file by another spelling too");
    assert_ne!(second, Some(known));
    assert_eq!(library.stats(&all_kinds()).unwrap().count, count + 1);
    // The later entry for the same file replaces the earlier one's overrides.
    assert_eq!(library.track(known).unwrap().unwrap().title, "Halo");
}

#[test]
fn track_numbers_composer_and_compilation_overrides_win_and_survive_rereads() {
    let (_dir, path) = one_file();
    let mut library = Library::open_in_memory().unwrap();
    let overrides = TrackOverrides {
        composer: Some("Pinned Composer".into()),
        track_number: Some(7),
        track_count: Some(12),
        disc_number: Some(2),
        disc_count: Some(3),
        compilation: Some(true),
        ..TrackOverrides::default()
    };
    let id = added_ids(&mut library, &[(path.clone(), overrides)])[0];
    let pinned = |library: &Library| {
        let row = library.track(id).unwrap().unwrap();
        (
            row.composer,
            row.track_number,
            row.track_count,
            row.disc_number,
            row.disc_count,
            row.compilation,
        )
    };
    let expected = (
        "Pinned Composer".to_owned(),
        Some(7),
        Some(12),
        Some(2),
        Some(3),
        true,
    );
    assert_eq!(pinned(&library), expected);
    let album = &library.albums(&all_kinds()).unwrap()[0];
    assert!(album.compilation, "it files under Compilations");

    // The file's own tags change, so it is read again; the pinned values hold.
    let mut tagged = lofty::read_from_path(&path).unwrap();
    let tag = tagged.primary_tag_mut().unwrap();
    tag.insert_text(ItemKey::Composer, "File Composer".into());
    tag.insert_text(ItemKey::TrackNumber, "1".into());
    tag.insert_text(ItemKey::DiscNumber, "1".into());
    tag.insert_text(ItemKey::FlagCompilation, "0".into());
    tag.set_title("Retagged".into());
    tagged.save_to_path(&path, WriteOptions::default()).unwrap();
    let summary = library
        .add_paths(&[path.parent().unwrap().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(summary.updated, 1);
    assert_eq!(library.track(id).unwrap().unwrap().title, "Retagged");
    assert_eq!(pinned(&library), expected);
}

#[test]
fn overrides_json_uses_camel_case_and_every_field_is_optional() {
    let parsed: TrackOverrides = serde_json::from_str(
        r#"{"kind":"podcast","albumArtist":"A","dateAdded":5,"trackNumber":3,"discCount":2,"compilation":true,"composer":"C"}"#,
    )
    .unwrap();
    assert_eq!(
        (
            parsed.track_number,
            parsed.disc_count,
            parsed.compilation,
            parsed.composer.as_deref()
        ),
        (Some(3), Some(2), Some(true), Some("C"))
    );

    assert_eq!(parsed.kind, Some(MediaKind::Podcast));
    assert_eq!(
        (parsed.album_artist.as_deref(), parsed.date_added),
        (Some("A"), Some(5))
    );
    assert_eq!(parsed.title, None);
    assert_eq!(
        serde_json::from_str::<TrackOverrides>("{}").unwrap(),
        TrackOverrides::default()
    );
}
