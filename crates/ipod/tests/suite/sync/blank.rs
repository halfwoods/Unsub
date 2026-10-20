//! The first sync to an iPod that has no database: one that was just restored, or never synced.

use std::fs;

use unsub_ipod::{ChecksumCheck, read_play_counts_for_sync_back, verify_roundtrip};

use super::faults::apply_with;
use super::*;
use crate::common::db::u64_at;

/// A classic 7G with its firmware's own files and music folders but no iTunesDB.
fn blank_classic() -> FakeIpod {
    let fake = FakeIpod::new("RESTORED");
    fake.write_sysinfo("xC293", "0x000A27001A2B3C4D");
    fs::create_dir_all(fake.mount.join("iPod_Control/Music/F01")).unwrap();
    fake
}

#[test]
fn an_ipod_without_a_database_reads_as_empty() {
    let fake = blank_classic();

    let ipod = opened(&fake);
    let database = read_database(&ipod).unwrap();

    assert_eq!(ipod.name, "RESTORED");
    assert!(database.tracks.is_empty() && database.playlists.is_empty());
    assert!(read_play_counts_for_sync_back(&ipod).unwrap().is_empty());
}

#[test]
fn the_first_sync_writes_a_database_the_ipod_can_read_back() {
    let fake = blank_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 1, "One"),
        library_track(library.path(), 2, "Two"),
    ];
    let playlists = [SyncPlaylist {
        name: "Mine".into(),
        tracks: vec![2, 1],
    }];

    let (plan, _) = sync(&fake, &tracks, &playlists);

    assert_eq!(plan.add, [1, 2]);
    let ipod = opened(&fake);
    let database = read_database(&ipod).unwrap();
    let titles: Vec<&str> = database.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["One", "Two"]);
    let master = database.playlists.iter().find(|p| p.is_master).unwrap();
    assert_eq!(master.name, "RESTORED");
    assert_eq!(master.track_ids.len(), 2);
    let mine = database
        .playlists
        .iter()
        .find(|p| p.name == "Mine")
        .unwrap();
    assert_eq!(mine.track_ids.len(), 2);
    assert_eq!(music_files(&fake).len(), 2);

    // It carries the checksum this iPod's GUID produces, and survives a rewrite unchanged.
    let report = verify_roundtrip(&ipod).unwrap();
    assert_eq!(report.checksum, ChecksumCheck::Verified);
    assert!(report.is_lossless(), "{report:#?}");
}

#[test]
fn the_first_sync_has_no_database_to_keep_a_copy_of() {
    let fake = blank_classic();
    let library = tempfile::tempdir().unwrap();

    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);

    let listing = itunes_dir_listing(&fake);
    assert!(listing.contains("iTunesDB") && listing.contains("Unsub.json"));
    assert!(
        !listing.iter().any(|name| name.contains("unsub-")),
        "{listing:?}"
    );

    // The second sync has one, and keeps it.
    sync(
        &fake,
        &[
            library_track(library.path(), 1, "One"),
            library_track(library.path(), 2, "Two"),
        ],
        &[],
    );
    assert!(itunes_file(&fake, "iTunesDB.unsub-backup").is_file());
    assert!(itunes_file(&fake, "iTunesDB.unsub-original").is_file());
}

#[test]
fn an_ipod_missing_even_the_itunes_folder_gets_it() {
    let fake = blank_classic();
    fs::remove_dir(fake.mount.join("iPod_Control/iTunes")).unwrap();
    let library = tempfile::tempdir().unwrap();

    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);

    assert_eq!(read_database(&opened(&fake)).unwrap().tracks.len(), 1);
}

#[test]
fn a_failed_first_sync_leaves_the_ipod_with_an_empty_database_and_no_music() {
    let fake = blank_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 1, "One"),
        library_track(library.path(), 2, "Two"),
    ];

    let result = apply_with(&fake, &tracks, &mut |track| {
        if track.library_id == 2 {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "the sync was cancelled",
            ))
        } else {
            Ok(track.source_path.clone().into())
        }
    });

    assert!(matches!(result, Err(Error::Io { .. })), "{result:?}");
    assert!(
        music_files(&fake).is_empty(),
        "copied music must be cleaned up"
    );
    // The empty database went in before any music, so nothing that is left reads as a lost one.
    let listing = itunes_dir_listing(&fake);
    assert!(listing.contains("iTunesDB"), "{listing:?}");
    assert!(
        !listing
            .iter()
            .any(|name| name.contains("unsub-") || name.contains("pending")),
        "{listing:?}"
    );
    assert!(read_database(&opened(&fake)).unwrap().tracks.is_empty());
    sync(&fake, &tracks, &[]);
    assert_eq!(read_database(&opened(&fake)).unwrap().tracks.len(), 2);
}

#[test]
fn music_left_by_a_first_sync_that_died_does_not_lock_the_ipod_out() {
    let fake = blank_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 1, "One")];

    // The process dies after the empty database went in and one song was copied.
    let result = apply_with(&fake, &tracks, &mut |_| Err(io::Error::other("killed")));
    assert!(result.is_err());
    fs::write(
        fake.mount.join("iPod_Control/Music/F00/DEAD.mp3"),
        b"half a song",
    )
    .unwrap();

    // Music on the iPod that its database does not list is nothing to refuse.
    sync(&fake, &tracks, &[]);

    assert_eq!(read_database(&opened(&fake)).unwrap().tracks.len(), 1);
}

#[test]
fn a_database_that_appears_during_the_first_sync_is_not_overwritten() {
    let fake = blank_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 1, "One")];
    let database = fake.mount.join(DB_FILE);

    let result = apply_with(&fake, &tracks, &mut |track| {
        // iTunes writes the iPod's first database while the music is being copied.
        fs::write(&database, b"iTunes got there first").unwrap();
        Ok(track.source_path.clone().into())
    });

    assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
    assert_eq!(fs::read(&database).unwrap(), b"iTunes got there first");
    assert!(music_files(&fake).is_empty());
}

fn music_folder_names(fake: &FakeIpod) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(fake.mount.join("iPod_Control/Music"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn an_ipod_with_no_music_folders_gets_the_number_its_model_has() {
    for (model, folders) in [("xC293", 50), ("xB518", 3)] {
        let fake = FakeIpod::new("RESTORED");
        fake.write_sysinfo(model, "0x000A27001A2B3C4D");
        fs::remove_dir_all(fake.mount.join("iPod_Control/Music")).unwrap();
        let library = tempfile::tempdir().unwrap();

        sync(&fake, &[library_track(library.path(), 1, "One")], &[]);

        let names = music_folder_names(&fake);
        let expected: Vec<String> = (0..folders).map(|n| format!("F{n:02}")).collect();
        assert_eq!(names, expected, "{model}");
        assert_eq!(music_files(&fake).len(), 1);
    }
}

#[test]
fn an_ipod_that_has_a_database_but_no_music_folders_is_still_refused() {
    let fake = fake_classic();
    fs::remove_dir_all(fake.mount.join("iPod_Control/Music")).unwrap();
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 1, "One")];
    let before = db_bytes(&fake);

    let plan = plan(&fake, &tracks, &[]);
    let result = apply_sync(
        &opened(&fake),
        &plan,
        &tracks,
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    );

    let Err(Error::SyncRefused(reason)) = result else {
        panic!("expected a refusal, got {result:?}")
    };
    assert!(reason.contains("F00-style"), "{reason}");
    assert_eq!(db_bytes(&fake), before);
    assert!(!fake.mount.join("iPod_Control/Music").exists());
}

fn assert_refused_as_having_lost_its_database(fake: &FakeIpod, why: &str) {
    let ipod = opened(fake);
    let current = read_database(&ipod).unwrap();
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 1, "One")];
    let forged = SyncPlan {
        add: vec![1],
        update: vec![],
        remove: vec![],
        bytes_needed: 4000,
        fits: true,
        removals_first: false,
    };
    let music = music_files(fake);

    let planned = plan_sync(&ipod, &current, &tracks, &[]);
    let applied = apply_sync(
        &ipod,
        &forged,
        &tracks,
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    );

    for result in [planned.map(drop), applied.map(drop)] {
        match result {
            Err(Error::SyncRefused(reason)) => {
                assert!(
                    reason.contains("music but no song database"),
                    "{why}: {reason}"
                );
            }
            other => panic!("{why}: expected a refusal, got {other:?}"),
        }
    }
    assert!(!itunes_dir_listing(fake).contains("iTunesDB"), "{why}");
    assert_eq!(music_files(fake), music, "{why}");
}

#[test]
fn an_ipod_with_music_but_no_database_is_refused_by_the_preview_and_the_sync() {
    type Setup = fn(&FakeIpod);
    let cases: [(&str, Setup); 3] = [
        ("music", |fake| {
            fs::write(fake.mount.join("iPod_Control/Music/F00/ABCD.mp3"), b"x").unwrap()
        }),
        ("a copy of the database", |fake| {
            fs::write(itunes_file(fake, "iTunesDB.unsub-backup"), b"x").unwrap()
        }),
        ("the first copy of the database", |fake| {
            fs::write(itunes_file(fake, "iTunesDB.unsub-original"), b"x").unwrap()
        }),
    ];
    for (what, setup) in cases {
        let fake = blank_classic();
        setup(&fake);

        assert_refused_as_having_lost_its_database(&fake, what);
    }
}

#[test]
fn a_dot_file_in_the_music_folders_is_not_music() {
    let fake = blank_classic();
    fs::write(fake.mount.join("iPod_Control/Music/F00/.DS_Store"), b"x").unwrap();
    let library = tempfile::tempdir().unwrap();

    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);

    assert_eq!(read_database(&opened(&fake)).unwrap().tracks.len(), 1);
}

fn blank_shuffle() -> FakeIpod {
    let fake = FakeIpod::new("SHUFFLE");
    fake.write_sysinfo("xB518", "0x000A270011223344");
    fake
}

#[test]
fn a_shuffle_with_a_song_list_but_no_database_is_refused_and_one_with_an_empty_list_is_not() {
    let fake = blank_shuffle();
    let library = tempfile::tempdir().unwrap();
    // A first sync leaves the iTunesSD with nothing in it; that is what a restore does too.
    sync(&fake, &[], &[]);
    fs::remove_file(itunes_file(&fake, "iTunesDB")).unwrap();
    fs::remove_file(itunes_file(&fake, "Unsub.json")).unwrap();
    for leftover in ["iTunesDB.unsub-backup", "iTunesDB.unsub-original"] {
        let _ = fs::remove_file(itunes_file(&fake, leftover));
    }
    assert!(itunes_file(&fake, "iTunesSD").is_file());

    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);
    assert_eq!(read_database(&opened(&fake)).unwrap().tracks.len(), 1);

    // Now its iTunesSD lists a song, and the database is gone.
    fs::remove_file(itunes_file(&fake, "iTunesDB")).unwrap();
    fs::remove_file(itunes_file(&fake, "Unsub.json")).unwrap();
    for file in music_files(&fake) {
        fs::remove_file(file).unwrap();
    }
    for leftover in ["iTunesDB.unsub-backup", "iTunesDB.unsub-original"] {
        let _ = fs::remove_file(itunes_file(&fake, leftover));
    }
    assert_refused_as_having_lost_its_database(&fake, "an iTunesSD that lists a song");
}

#[test]
fn the_database_a_first_sync_writes_carries_ids_and_the_zones_offset() {
    let fake = blank_classic();
    let library = tempfile::tempdir().unwrap();

    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);

    let bytes = db_bytes(&fake);
    assert_ne!(u64_at(&bytes, 0x18), 0, "database id");
    assert_ne!(u64_at(&bytes, 0x48), 0, "persistent id");
    assert_ne!(u64_at(&bytes, 0x18), u64_at(&bytes, 0x48));
    assert_eq!(&bytes[0x46..0x48], b"en");
    let offset = chrono::Local::now().offset().local_minus_utc();
    assert_eq!(
        i32::from_le_bytes(bytes[0x6c..0x70].try_into().unwrap()),
        offset
    );
    // Two blank iPods do not share ids.
    let other = blank_classic();
    sync(&other, &[library_track(library.path(), 2, "Two")], &[]);
    assert_ne!(u64_at(&db_bytes(&other), 0x48), u64_at(&bytes, 0x48));
}
