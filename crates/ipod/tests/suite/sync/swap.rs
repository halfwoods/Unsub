use std::fs::{self, File};

use antistreaming_ipod::{Error, SyncTrack, apply_sync, read_database};

use super::*;

/// A sparse file claiming `len` bytes: it has a real size to the sync but uses no disk, so
/// "the iPod is nearly full" can be staged on a machine with plenty of room.
fn claim_size(path: &Path, len: u64) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(len)
        .unwrap();
}

struct Swap {
    fake: FakeIpod,
    library: tempfile::TempDir,
    old_file: PathBuf,
    new: SyncTrack,
    tiny: PathBuf,
}

/// An iPod holding one track of this app's that "occupies a petabyte", and a library that wants it
/// replaced by another track of the same claimed size.
fn nearly_full(new_claims: u64) -> Swap {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let old = library_track(library.path(), 1, "Old");
    sync(&fake, std::slice::from_ref(&old), &[]);
    let old_file = track_path(&fake, "Old");
    claim_size(&old_file, 1 << 50);
    let new = library_track(library.path(), 2, "New");
    claim_size(&new.source_path, new_claims);
    let tiny = library.path().join("tiny.mp3");
    fs::write(&tiny, b"ID3 tiny").unwrap();
    Swap {
        fake,
        library,
        old_file,
        new,
        tiny,
    }
}

#[test]
fn a_swap_that_only_fits_once_the_old_track_is_gone_removes_it_first() {
    let swap = nearly_full(1 << 50);
    let before = db_bytes(&swap.fake);
    let wanted = [swap.new.clone()];

    let plan = plan(&swap.fake, &wanted, &[]);

    assert!(plan.fits && plan.removals_first, "{plan:?}");
    assert_eq!((plan.add.clone(), plan.remove.len()), (vec![2], 1));
    let mut order = Vec::new();
    let tiny = swap.tiny.clone();
    apply_sync(
        &opened(&swap.fake),
        &plan,
        &wanted,
        &[],
        &mut |_| Ok(tiny.clone()),
        &mut no_cover,
        &mut |progress| order.push(progress.stage),
    )
    .unwrap();

    let read = read_database(&opened(&swap.fake)).unwrap();
    assert!(read.tracks.iter().any(|t| t.title == "New"));
    assert!(read.tracks.iter().all(|t| t.title != "Old"));
    assert!(!swap.old_file.exists());
    // The removal was its own committed step, so the database was written twice...
    assert_eq!(
        order
            .iter()
            .filter(|stage| format!("{stage:?}") == "WritingDatabase")
            .count(),
        4,
        "two commits, each reporting start and finish"
    );
    // ...and the backup is still the database from before the sync, not the intermediate one.
    assert_eq!(
        fs::read(itunes_file(&swap.fake, "iTunesDB.antistreaming-backup")).unwrap(),
        before
    );
}

#[test]
fn a_swap_that_fits_without_removals_is_not_split() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &[library_track(library.path(), 1, "Old")], &[]);

    let plan = plan(&fake, &[library_track(library.path(), 2, "New")], &[]);

    assert!(plan.fits && !plan.removals_first);
}

#[test]
fn a_failure_after_the_removal_leaves_the_ipod_with_fewer_tracks_never_more() {
    let swap = nearly_full(1 << 50);
    let wanted = [swap.new.clone()];
    let plan = plan(&swap.fake, &wanted, &[]);
    let before = db_bytes(&swap.fake);
    let files_before = music_files(&swap.fake);

    let result = apply_sync(
        &opened(&swap.fake),
        &plan,
        &wanted,
        &[],
        &mut |_| Err(std::io::Error::other("transcoder crashed")),
        &mut no_cover,
        &mut |_| {},
    );

    let Err(Error::AfterRemovals { removed, source }) = result else {
        panic!("expected AfterRemovals, got {result:?}");
    };
    assert_eq!(removed, 1);
    assert!(matches!(*source, Error::Io { .. }), "{source:?}");
    let read = read_database(&opened(&swap.fake)).unwrap();
    assert_eq!(
        read.tracks.len(),
        5,
        "only the five tracks iTunes put there remain"
    );
    assert!(!swap.old_file.exists());
    assert_eq!(
        music_files(&swap.fake).len() + 1,
        files_before.len(),
        "one file fewer, none extra"
    );
    assert_eq!(
        fs::read(itunes_file(&swap.fake, "iTunesDB.antistreaming-backup")).unwrap(),
        before
    );
    // The next sync picks up where this one stopped (here the library's file has shrunk to its
    // real size).
    claim_size(&swap.new.source_path, 4000);
    let (_, written) = {
        let again = self::plan(&swap.fake, &wanted, &[]);
        assert_eq!(again.add, [2]);
        assert!(again.remove.is_empty());
        let tiny = swap.tiny.clone();
        let written = apply_sync(
            &opened(&swap.fake),
            &again,
            &wanted,
            &[],
            &mut |_| Ok(tiny.clone()),
            &mut no_cover,
            &mut |_| {},
        )
        .unwrap();
        (again, written)
    };
    assert!(written.tracks.iter().any(|t| t.title == "New"));
    drop(swap.library);
}

#[test]
fn a_swap_too_big_even_after_removals_is_refused_with_nothing_changed() {
    let swap = nearly_full(1 << 51);
    let wanted = [swap.new.clone()];
    let before = db_bytes(&swap.fake);
    let files = music_files(&swap.fake);

    let plan = plan(&swap.fake, &wanted, &[]);
    let result = apply_sync(
        &opened(&swap.fake),
        &plan,
        &wanted,
        &[],
        &mut |_| Ok(swap.tiny.clone()),
        &mut no_cover,
        &mut |_| {},
    );

    assert!(!plan.fits && !plan.removals_first);
    assert!(
        matches!(result, Err(Error::InsufficientSpace { .. })),
        "{result:?}"
    );
    assert_eq!(db_bytes(&swap.fake), before);
    assert_eq!(music_files(&swap.fake), files);
    assert!(
        swap.old_file.exists(),
        "the track to be removed is still there"
    );
}
