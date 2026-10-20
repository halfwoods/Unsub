//! A sync that is stopped, or fails, partway through a long copy keeps the songs already
//! committed: a batch is committed every 500 songs, and a stop commits what has been copied.

use std::collections::HashSet;
use std::fs;
use std::io;

use unsub_ipod::{
    ChecksumCheck, Error, Prepared, SyncPlaylist, SyncStage, SyncTrack, apply_sync, read_database,
    verify_roundtrip,
};

use super::faults::apply_with;
use super::*;

const BATCH: i64 = 500;

fn many_tracks(library: &Path, count: i64) -> Vec<SyncTrack> {
    (1..=count)
        .map(|id| library_track(library, id, &format!("Song {id:03}")))
        .collect()
}

#[test]
fn stopping_commits_the_songs_copied_so_far_and_the_next_sync_carries_on() {
    let fake = fake_classic();
    let original = db_bytes(&fake);
    let library = tempfile::tempdir().unwrap();
    let tracks = many_tracks(library.path(), 5);
    let playlists = [SyncPlaylist {
        name: "Mine".into(),
        tracks: vec![5, 4, 3, 2, 1],
    }];
    let plan = plan(&fake, &tracks, &playlists);

    let written = apply_sync(
        &opened(&fake),
        &plan,
        &tracks,
        &playlists,
        &mut |track| {
            Ok(if track.library_id == 3 {
                Prepared::Stop
            } else {
                track.source_path.clone().into()
            })
        },
        &mut no_cover,
        &mut |_| {},
    )
    .unwrap();

    assert_eq!(written.tracks.len(), 5 + 2);
    assert_eq!(music_files(&fake).len(), 2);
    let ipod = opened(&fake);
    let read = read_database(&ipod).unwrap();
    let mine = read.playlists.iter().find(|p| p.name == "Mine").unwrap();
    assert_eq!(mine.track_ids.len(), 2, "it lists the songs that are there");
    assert_eq!(
        verify_roundtrip(&ipod).unwrap().checksum,
        ChecksumCheck::Verified
    );
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesDB.unsub-backup")).unwrap(),
        original
    );
    assert!(
        !itunes_dir_listing(&fake)
            .iter()
            .any(|f| f.ends_with(".unsub-tmp"))
    );

    // The rest are still to do, and only the rest.
    let again = super::plan(&fake, &tracks, &playlists);
    assert_eq!(again.add, [3, 4, 5]);
    sync(&fake, &tracks, &playlists);
    assert_eq!(read_database(&opened(&fake)).unwrap().tracks.len(), 5 + 5);
}

#[test]
fn stopping_before_anything_is_copied_changes_nothing_that_matters() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = many_tracks(library.path(), 2);
    let before = read_database(&opened(&fake)).unwrap();

    let result = apply_with(&fake, &tracks, &mut |_| Ok(Prepared::Stop));

    assert!(result.is_ok(), "{result:?}");
    assert!(music_files(&fake).is_empty());
    assert_eq!(read_database(&opened(&fake)).unwrap().tracks, before.tracks);
}

#[test]
fn a_long_sync_commits_every_five_hundred_songs() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = many_tracks(library.path(), BATCH + 1);
    let playlists = [SyncPlaylist {
        name: "All".into(),
        tracks: (1..=BATCH + 1).rev().collect(),
    }];
    let plan = plan(&fake, &tracks, &playlists);
    let mut commits = 0;

    apply_sync(
        &opened(&fake),
        &plan,
        &tracks,
        &playlists,
        &mut copy_as_is,
        &mut no_cover,
        &mut |progress| {
            if progress.stage == SyncStage::WritingDatabase && progress.done == 1 {
                commits += 1;
            }
        },
    )
    .unwrap();

    assert_eq!(commits, 2, "one full batch, then the last song");
    let read = read_database(&opened(&fake)).unwrap();
    assert_eq!(read.tracks.len(), 5 + 501);
    let ids: HashSet<u32> = read.tracks.iter().map(|t| t.id).collect();
    let dbids: HashSet<u64> = read.tracks.iter().map(|t| t.dbid).collect();
    assert_eq!((ids.len(), dbids.len()), (506, 506), "no id is used twice");
    let all = read.playlists.iter().find(|p| p.name == "All").unwrap();
    assert_eq!(all.track_ids.len(), 501);
    let titles: Vec<&str> = all
        .track_ids
        .iter()
        .map(|id| {
            read.tracks
                .iter()
                .find(|t| t.id == *id)
                .unwrap()
                .title
                .as_str()
        })
        .collect();
    assert_eq!(titles.first(), Some(&"Song 501"));
    assert_eq!(titles.last(), Some(&"Song 001"));
    assert_eq!(
        verify_roundtrip(&opened(&fake)).unwrap().checksum,
        ChecksumCheck::Verified
    );
    // The copy kept is the database as it was before the sync, not as a batch left it.
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesDB.unsub-backup"))
            .unwrap()
            .len(),
        fs::read(itunes_file(&fake, "iTunesDB.unsub-original"))
            .unwrap()
            .len()
    );
}

#[test]
fn a_failure_after_a_batch_keeps_that_batch_and_reports_it() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = many_tracks(library.path(), BATCH + 1);

    let result = apply_with(&fake, &tracks, &mut |track| {
        if track.library_id == BATCH + 1 {
            Err(io::Error::other("the cable came out"))
        } else {
            Ok(track.source_path.clone().into())
        }
    });

    let Err(Error::Partial { added, source }) = result else {
        panic!("expected Partial, got {result:?}");
    };
    assert_eq!(added, 500);
    assert!(matches!(*source, Error::Io { .. }), "{source:?}");
    let ipod = opened(&fake);
    assert_eq!(read_database(&ipod).unwrap().tracks.len(), 5 + 500);
    assert_eq!(music_files(&fake).len(), 500);
    assert_eq!(
        verify_roundtrip(&ipod).unwrap().checksum,
        ChecksumCheck::Verified
    );
    assert!(
        !itunes_dir_listing(&fake)
            .iter()
            .any(|f| f.ends_with(".unsub-tmp"))
    );

    // The next sync does only what is left.
    let again = super::plan(&fake, &tracks, &[]);
    assert_eq!(again.add, [BATCH + 1]);
    sync(&fake, &tracks, &[]);
    assert_eq!(read_database(&opened(&fake)).unwrap().tracks.len(), 5 + 501);
}

#[test]
fn a_failure_in_the_first_batch_is_not_called_partial() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = many_tracks(library.path(), 3);

    let result = apply_with(&fake, &tracks, &mut |_| Err(io::Error::other("no")));

    assert!(matches!(result, Err(Error::Io { .. })), "{result:?}");
}

#[test]
fn a_song_that_comes_out_bigger_than_the_room_left_runs_the_ipod_out_of_space() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = many_tracks(library.path(), 2);
    let huge = library.path().join("huge.mp3");
    fs::write(&huge, b"ID3").unwrap();
    claim_size(&huge, HUGE);
    let before = db_bytes(&fake);

    // The plan was made for the small files; the "converted" one comes out far bigger.
    let result = apply_with(&fake, &tracks, &mut |track| {
        Ok(if track.library_id == 2 {
            huge.clone().into()
        } else {
            track.source_path.clone().into()
        })
    });

    let Err(error @ Error::OutOfSpace { .. }) = result else {
        panic!("expected OutOfSpace, got {result:?}");
    };
    assert!(
        error
            .to_string()
            .starts_with("the iPod ran out of space while copying"),
        "{error}"
    );
    assert_eq!(db_bytes(&fake), before);
    assert!(music_files(&fake).is_empty());
}

/// An iPod holding three songs this app added, and a library that wants five others instead.
fn swap_three_for_five() -> (FakeIpod, tempfile::TempDir, Vec<SyncTrack>) {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &many_tracks(library.path(), 3), &[]);
    let wanted = (10..15)
        .map(|id| library_track(library.path(), id, &format!("New {id}")))
        .collect();
    (fake, library, wanted)
}

#[test]
fn stopping_does_not_remove_the_songs_the_plan_would_have() {
    let (fake, _library, wanted) = swap_three_for_five();
    let old_files = music_files(&fake);
    let plan = plan(&fake, &wanted, &[]);
    assert_eq!((plan.add.len(), plan.remove.len()), (5, 3));

    apply_sync(
        &opened(&fake),
        &plan,
        &wanted,
        &[],
        &mut |track| {
            Ok(if track.library_id == 12 {
                Prepared::Stop
            } else {
                track.source_path.clone().into()
            })
        },
        &mut no_cover,
        &mut |_| {},
    )
    .unwrap();

    let titles: Vec<String> = read_database(&opened(&fake))
        .unwrap()
        .tracks
        .into_iter()
        .map(|track| track.title)
        .collect();
    for old in ["Song 001", "Song 002", "Song 003", "New 10", "New 11"] {
        assert!(titles.iter().any(|t| t == old), "{old} in {titles:?}");
    }
    assert!(
        old_files.is_subset(&music_files(&fake)),
        "their files are still there"
    );
    // The sync that is let finish does what it was going to.
    sync(&fake, &wanted, &[]);
    let titles: Vec<String> = read_database(&opened(&fake))
        .unwrap()
        .tracks
        .into_iter()
        .map(|track| track.title)
        .collect();
    assert!(!titles.iter().any(|t| t.starts_with("Song ")), "{titles:?}");
    assert!(music_files(&fake).is_disjoint(&old_files));
}

#[test]
fn a_failure_after_a_checkpoint_does_not_remove_the_songs_the_plan_would_have() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &many_tracks(library.path(), 3), &[]);
    let old_files = music_files(&fake);
    let wanted: Vec<SyncTrack> = (100..=100 + BATCH)
        .map(|id| library_track(library.path(), id, &format!("New {id}")))
        .collect();

    let result = apply_with(&fake, &wanted, &mut |track| {
        if track.library_id == 100 + BATCH {
            Err(io::Error::other("the cable came out"))
        } else {
            Ok(track.source_path.clone().into())
        }
    });

    assert!(
        matches!(result, Err(Error::Partial { added: 500, .. })),
        "{result:?}"
    );
    let read = read_database(&opened(&fake)).unwrap();
    assert_eq!(read.tracks.len(), 5 + 3 + 500, "the three are still there");
    assert!(old_files.is_subset(&music_files(&fake)));
}

fn pending_file(fake: &FakeIpod) -> PathBuf {
    itunes_file(fake, "Unsub.pending")
}

#[test]
fn the_music_being_copied_is_listed_until_its_batch_is_committed() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = many_tracks(library.path(), 3);
    let mut listed_at_the_third = Vec::new();

    apply_with(&fake, &tracks, &mut |track| {
        if track.library_id == 3 {
            listed_at_the_third = fs::read_to_string(pending_file(&fake))
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect();
        }
        Ok(track.source_path.clone().into())
    })
    .unwrap();

    assert_eq!(listed_at_the_third.len(), 2, "{listed_at_the_third:?}");
    assert!(
        listed_at_the_third
            .iter()
            .all(|line| line.starts_with("iPod_Control/Music/F") && line.ends_with(".mp3")),
        "{listed_at_the_third:?}"
    );
    assert!(!pending_file(&fake).exists());
}

#[test]
fn music_a_dead_sync_was_copying_is_swept_at_the_next_sync_and_nothing_else_is() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &many_tracks(library.path(), 1), &[]);
    let kept = music_files(&fake).into_iter().next().unwrap();
    let music = fake.mount.join("iPod_Control/Music/F01");
    let (orphan, innocent) = (music.join("DEAD.mp3"), music.join("MINE.mp3"));
    fs::write(&orphan, b"half a song").unwrap();
    fs::write(&innocent, b"not on the list").unwrap();
    let relative = |file: &Path| {
        file.strip_prefix(&fake.mount)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };
    // What the list names: the orphan; a song the database does list; a file that is not music
    // at all; and a path trying to climb out of the music folders.
    let list = [
        relative(&orphan),
        relative(&kept),
        "iPod_Control/iTunes/iTunesDB".to_owned(),
        "iPod_Control/Music/../iTunes/Unsub.json".to_owned(),
    ];
    fs::write(pending_file(&fake), list.join("\n")).unwrap();
    let database = db_bytes(&fake);
    let mapping = fs::read(itunes_file(&fake, "Unsub.json")).unwrap();

    sync(&fake, &many_tracks(library.path(), 1), &[]);

    assert!(!orphan.exists(), "the orphan is swept");
    assert!(kept.exists() && innocent.exists());
    assert!(!pending_file(&fake).exists());
    assert_eq!(
        db_bytes(&fake),
        database,
        "nothing needed changing in the database"
    );
    assert_eq!(fs::read(itunes_file(&fake, "Unsub.json")).unwrap(), mapping);
}
