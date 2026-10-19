use std::fs;
use std::io;

use antistreaming_ipod::{Error, SyncTrack, apply_sync, read_database};

use super::*;

/// What a failed sync must not have changed.
struct Untouched {
    database: Vec<u8>,
    music: BTreeSet<PathBuf>,
    itunes_dir: BTreeSet<String>,
}

impl Untouched {
    fn capture(fake: &FakeIpod) -> Self {
        Self {
            database: db_bytes(fake),
            music: music_files(fake),
            itunes_dir: itunes_dir_listing(fake),
        }
    }

    fn assert_holds(&self, fake: &FakeIpod) {
        assert_eq!(
            db_bytes(fake),
            self.database,
            "the database must be byte-for-byte what it was"
        );
        assert_eq!(
            music_files(fake),
            self.music,
            "copied music must be cleaned up"
        );
        assert_eq!(
            itunes_dir_listing(fake),
            self.itunes_dir,
            "no temporary files may be left"
        );
        // And it still opens and reads.
        assert_eq!(read_database(&opened(fake)).unwrap().tracks.len(), 5);
    }
}

fn three_tracks(library: &Path) -> Vec<SyncTrack> {
    vec![
        library_track(library, 1, "One"),
        library_track(library, 2, "Two"),
        library_track(library, 3, "Three"),
    ]
}

fn apply_with(
    fake: &FakeIpod,
    tracks: &[SyncTrack],
    prepare: &mut dyn FnMut(&SyncTrack) -> io::Result<PathBuf>,
) -> Result<Database, Error> {
    let plan = plan(fake, tracks, &[]);
    apply_sync(&opened(fake), &plan, tracks, &[], prepare, &mut |_| {})
}

#[test]
fn a_failure_while_preparing_the_second_track_changes_nothing() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = three_tracks(library.path());
    let untouched = Untouched::capture(&fake);
    let mut calls = 0;

    let result = apply_with(&fake, &tracks, &mut |track| {
        calls += 1;
        if calls == 2 {
            Err(io::Error::other("transcoder crashed"))
        } else {
            Ok(track.source_path.clone())
        }
    });

    assert!(matches!(result, Err(Error::Io { .. })), "{result:?}");
    assert_eq!(calls, 2, "the sync stops at the first failure");
    untouched.assert_holds(&fake);
}

#[test]
fn a_source_file_that_vanishes_mid_sync_changes_nothing() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = three_tracks(library.path());
    let untouched = Untouched::capture(&fake);

    let result = apply_with(&fake, &tracks, &mut |track| {
        if track.library_id == 3 {
            fs::remove_file(&track.source_path).unwrap();
        }
        Ok(track.source_path.clone())
    });

    assert!(result.is_err());
    untouched.assert_holds(&fake);
}

#[test]
fn a_failure_writing_the_new_database_keeps_the_old_one_and_removes_copied_music() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = three_tracks(library.path());
    // A directory where the temporary database must go makes writing it fail, after the music has
    // already been copied.
    fs::create_dir(itunes_file(&fake, "iTunesDB.antistreaming-tmp")).unwrap();
    let untouched = Untouched::capture(&fake);

    let result = apply_with(&fake, &tracks, &mut copy_as_is);

    assert!(matches!(result, Err(Error::Io { .. })), "{result:?}");
    untouched.assert_holds(&fake);
}

#[test]
fn a_failure_keeping_the_backup_stops_before_the_database_is_replaced() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = three_tracks(library.path());
    fs::create_dir(itunes_file(&fake, "iTunesDB.antistreaming-backup")).unwrap();
    let untouched = Untouched::capture(&fake);

    let result = apply_with(&fake, &tracks, &mut copy_as_is);

    assert!(result.is_err());
    untouched.assert_holds(&fake);
}

#[test]
fn a_failure_staging_the_bookkeeping_file_stops_before_the_database_is_replaced() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = three_tracks(library.path());
    fs::create_dir(itunes_file(&fake, "AntiStreaming.json.antistreaming-tmp")).unwrap();
    let untouched = Untouched::capture(&fake);

    let result = apply_with(&fake, &tracks, &mut copy_as_is);

    assert!(result.is_err());
    untouched.assert_holds(&fake);
}

#[test]
fn leftovers_from_an_interrupted_sync_do_not_get_in_the_way() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    fs::write(
        itunes_file(&fake, "iTunesDB.antistreaming-tmp"),
        b"half a database",
    )
    .unwrap();
    fs::write(
        itunes_file(&fake, "AntiStreaming.json.antistreaming-tmp"),
        b"{",
    )
    .unwrap();

    sync(&fake, &three_tracks(library.path()), &[]);

    let listing = itunes_dir_listing(&fake);
    assert!(!listing.contains("iTunesDB.antistreaming-tmp"));
    assert!(!listing.contains("AntiStreaming.json.antistreaming-tmp"));
    assert_eq!(read_database(&opened(&fake)).unwrap().tracks.len(), 8);
}

#[test]
fn a_database_changed_by_someone_else_mid_sync_is_not_overwritten() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = three_tracks(library.path());
    let database = fake.mount.join(DB_FILE);
    let music_before = music_files(&fake);

    let result = apply_with(&fake, &tracks, &mut |track| {
        if track.library_id == 3 {
            // iTunes (or anything else) rewrites the database while music is being copied.
            let mut bytes = fs::read(&database).unwrap();
            bytes.push(0);
            fs::write(&database, bytes).unwrap();
        }
        Ok(track.source_path.clone())
    });

    assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
    assert_eq!(
        fs::read(&database).unwrap().last(),
        Some(&0),
        "their version is still there"
    );
    assert_eq!(music_files(&fake), music_before);
}

#[test]
fn a_sync_that_does_not_fit_is_refused_before_anything_is_copied() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let huge = library_track(library.path(), 1, "Enormous");
    // A sparse file claiming a petabyte: no disk is used, but the size is real.
    fs::File::options()
        .write(true)
        .open(&huge.source_path)
        .unwrap()
        .set_len(1 << 50)
        .unwrap();
    let tracks = [huge];
    let untouched = Untouched::capture(&fake);

    let plan = plan(&fake, &tracks, &[]);
    let result = apply_sync(
        &opened(&fake),
        &plan,
        &tracks,
        &[],
        &mut copy_as_is,
        &mut |_| {},
    );

    assert_eq!(plan.bytes_needed, 1 << 50);
    assert!(!plan.fits);
    assert!(
        matches!(result, Err(Error::InsufficientSpace { .. })),
        "{result:?}"
    );
    untouched.assert_holds(&fake);
}

#[test]
fn the_files_of_removed_tracks_survive_a_sync_that_fails() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let old = library_track(library.path(), 1, "Old");
    sync(&fake, std::slice::from_ref(&old), &[]);
    let old_file = read_database(&opened(&fake))
        .unwrap()
        .tracks
        .iter()
        .find(|t| t.title == "Old")
        .unwrap()
        .path
        .clone();
    fs::create_dir(itunes_file(&fake, "iTunesDB.antistreaming-tmp")).unwrap();
    let database = db_bytes(&fake);
    let wanted = [library_track(library.path(), 2, "New")];

    let result = apply_with_current(&fake, &wanted);

    assert!(result.is_err());
    assert!(
        old_file.exists(),
        "nothing is deleted until the new database is in place"
    );
    assert_eq!(db_bytes(&fake), database);
    assert!(
        read_database(&opened(&fake))
            .unwrap()
            .tracks
            .iter()
            .any(|t| t.title == "Old")
    );
}

fn apply_with_current(fake: &FakeIpod, tracks: &[SyncTrack]) -> Result<Database, Error> {
    apply_with(fake, tracks, &mut copy_as_is)
}

#[test]
fn a_prepared_file_the_ipod_cannot_play_is_refused_before_anything_is_copied() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = three_tracks(library.path());
    let flac = library.path().join("2.flac");
    fs::write(&flac, b"fLaC").unwrap();
    let untouched = Untouched::capture(&fake);

    let result = apply_with(&fake, &tracks, &mut |track| {
        Ok(if track.library_id == 2 {
            flac.clone()
        } else {
            track.source_path.clone()
        })
    });

    let Err(Error::Io { source, .. }) = result else {
        panic!("expected an I/O refusal")
    };
    assert!(source.to_string().contains(".flac"), "{source}");
    untouched.assert_holds(&fake);
}

/// Deterministic xorshift so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % bound as u64).unwrap()
    }
}

#[test]
fn a_damaged_database_is_either_refused_untouched_or_rewritten_validly() {
    let fake = fake_classic();
    let pristine = db_bytes(&fake);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut refused, mut rewritten) = (0, 0);

    for _ in 0..150 {
        let mut damaged = pristine.clone();
        for _ in 0..=rng.below(4) {
            let at = rng.below(damaged.len());
            if rng.below(2) == 0 {
                damaged[at] = u8::try_from(rng.next() & 0xff).unwrap();
            } else if at + 4 <= damaged.len() {
                let value = [0u32, 1, 12, 0xffff_ffff, 0x7fff_ffff][rng.below(5)];
                damaged[at..at + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        fake.write_itunesdb(&damaged);
        let Ok(ipod) = antistreaming_ipod::open(&fake.mount) else {
            continue;
        };
        let empty = SyncPlan {
            add: vec![],
            update: vec![],
            remove: vec![],
            bytes_needed: 0,
            fits: true,
        };

        match apply_sync(&ipod, &empty, &[], &[], &mut copy_as_is, &mut |_| {}) {
            Err(_) => {
                refused += 1;
                assert_eq!(
                    db_bytes(&fake),
                    damaged,
                    "a refused sync must leave the database alone"
                );
            }
            Ok(_) => {
                rewritten += 1;
                assert!(
                    read_database(&ipod).is_ok(),
                    "a rewritten database must read back"
                );
            }
        }
    }
    assert!(
        refused > 0 && rewritten > 0,
        "refused {refused}, rewritten {rewritten}"
    );
}
