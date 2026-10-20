use unsub_ipod::{Error, SyncPlaylist, read_database, restore_backup};

use super::*;
use crate::common::artwork::png;
use crate::common::db::{Track, play_counts_file};
use crate::common::shuffle::parse_fixed;

fn backup_file(fake: &FakeIpod) -> PathBuf {
    itunes_file(fake, "iTunesDB.unsub-backup")
}

fn mapping_json(fake: &FakeIpod) -> serde_json::Value {
    serde_json::from_slice(&fs::read(itunes_file(fake, "Unsub.json")).unwrap()).unwrap()
}

fn mapped_library_ids(fake: &FakeIpod) -> Vec<i64> {
    mapping_json(fake)["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| track["libraryId"].as_i64().unwrap())
        .collect()
}

fn artwork_key(fake: &FakeIpod, library_id: i64) -> Option<u64> {
    mapping_json(fake)["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|track| track["libraryId"] == library_id)
        .and_then(|track| track["artworkKey"].as_u64())
}

/// A classic that has had two syncs: the first added track 1, the second track 2 and a playlist.
/// The backup is the database as the first one left it, which carries the checksum this device
/// needs (the plain fixture does not).
struct TwoSyncs {
    fake: FakeIpod,
    one: SyncTrack,
    two: SyncTrack,
    mix: SyncPlaylist,
    after_first: Vec<u8>,
    music_after_first: BTreeSet<PathBuf>,
    _library: tempfile::TempDir,
}

fn two_syncs() -> TwoSyncs {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let one = library_track(library.path(), 1, "One");
    let two = library_track(library.path(), 2, "Two");
    let mix = SyncPlaylist {
        name: "Mix".to_owned(),
        tracks: vec![1, 2],
    };
    sync(&fake, std::slice::from_ref(&one), &[]);
    let after_first = db_bytes(&fake);
    let music_after_first = music_files(&fake);
    sync(
        &fake,
        &[one.clone(), two.clone()],
        std::slice::from_ref(&mix),
    );
    TwoSyncs {
        fake,
        one,
        two,
        mix,
        after_first,
        music_after_first,
        _library: library,
    }
}

fn assert_refused_untouched(fake: &FakeIpod, expected: fn(&Error) -> bool) {
    let database = db_bytes(fake);
    let mapping = fs::read(itunes_file(fake, "Unsub.json")).ok();
    let music = music_files(fake);
    let listing = itunes_dir_listing(fake);

    let result = restore_backup(&opened(fake));

    match result {
        Err(error) if expected(&error) => {}
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(db_bytes(fake), database);
    assert_eq!(fs::read(itunes_file(fake, "Unsub.json")).ok(), mapping);
    assert_eq!(music_files(fake), music);
    assert_eq!(itunes_dir_listing(fake), listing);
}

fn is_refusal(error: &Error) -> bool {
    matches!(error, Error::SyncRefused(_))
}

#[test]
fn restoring_puts_back_the_previous_database_exactly_and_the_next_sync_sees_the_truth() {
    let state = two_syncs();
    let fake = &state.fake;
    assert_ne!(db_bytes(fake), state.after_first);
    assert_eq!(mapped_library_ids(fake), [1, 2]);

    restore_backup(&opened(fake)).unwrap();

    assert_eq!(db_bytes(fake), state.after_first, "byte for byte");
    assert_eq!(
        music_files(fake),
        state.music_after_first,
        "the music the undone sync added is gone and the rest is kept"
    );
    assert_eq!(mapped_library_ids(fake), [1]);
    assert!(
        mapping_json(fake)["playlists"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let read = read_database(&opened(fake)).unwrap();
    assert!(read.playlists.iter().all(|playlist| playlist.name != "Mix"));
    assert_eq!(read.tracks.len(), 6);

    let both = [state.one.clone(), state.two.clone()];
    let wanted = std::slice::from_ref(&state.mix);
    let plan = plan(fake, &both, wanted);
    assert_eq!(
        (plan.add.clone(), plan.update.clone(), plan.remove.clone()),
        (vec![2], vec![], vec![])
    );
    sync(fake, &both, wanted);
    let again = read_database(&opened(fake)).unwrap();
    assert_eq!(again.tracks.len(), 7, "track 2 comes back once");
    assert!(
        again
            .playlists
            .iter()
            .any(|playlist| playlist.name == "Mix")
    );
}

#[test]
fn restoring_twice_changes_nothing_the_second_time() {
    let state = two_syncs();
    restore_backup(&opened(&state.fake)).unwrap();
    let database = db_bytes(&state.fake);
    let mapping = fs::read(itunes_file(&state.fake, "Unsub.json")).unwrap();
    let music = music_files(&state.fake);

    restore_backup(&opened(&state.fake)).unwrap();

    assert_eq!(db_bytes(&state.fake), database);
    assert_eq!(
        fs::read(itunes_file(&state.fake, "Unsub.json")).unwrap(),
        mapping
    );
    assert_eq!(music_files(&state.fake), music);
}

#[test]
fn a_database_too_damaged_to_read_is_still_replaced() {
    let state = two_syncs();
    fs::write(
        state.fake.mount.join(DB_FILE),
        b"mhbd and then nothing useful",
    )
    .unwrap();

    restore_backup(&opened(&state.fake)).unwrap();

    assert_eq!(db_bytes(&state.fake), state.after_first);
    assert_eq!(mapped_library_ids(&state.fake), [1]);
    assert_eq!(read_database(&opened(&state.fake)).unwrap().tracks.len(), 6);
}

/// The database being replaced has a track the backup lacks, with its file on the iPod; the
/// bookkeeping file does or does not say this app added it.
fn extra_track_setup(added_by_app: bool, path: &str) -> (FakeIpod, PathBuf) {
    let state = two_syncs();
    let fake = state.fake;
    let mut extra = Track::new(9, "Extra");
    extra.path = path.to_owned();
    let mut database = sample_database();
    database.tracks.push(extra.clone());
    let backup = state.after_first.clone();
    fs::write(backup_file(&fake), &backup).unwrap();
    fake.write_itunesdb(&database.build());
    let file = fake
        .mount
        .join(path.trim_start_matches(':').replace(':', "/"));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, b"music").unwrap();
    let tracks = if added_by_app {
        format!(r#"[{{"libraryId":9,"dbid":{}}}]"#, extra.dbid)
    } else {
        "[]".to_owned()
    };
    fs::write(
        itunes_file(&fake, "Unsub.json"),
        format!(r#"{{"version":1,"tracks":{tracks},"playlists":[],"artwork":[]}}"#),
    )
    .unwrap();
    (fake, file)
}

#[test]
fn music_the_app_did_not_add_is_never_deleted() {
    let (fake, file) = extra_track_setup(false, ":iPod_Control:Music:F01:EXTR.mp3");

    restore_backup(&opened(&fake)).unwrap();

    assert!(
        file.exists(),
        "iTunes' music stays even when its entry goes"
    );
}

#[test]
fn music_the_app_added_and_the_restored_database_does_not_list_is_deleted() {
    let (fake, file) = extra_track_setup(true, ":iPod_Control:Music:F01:EXTR.mp3");

    restore_backup(&opened(&fake)).unwrap();

    assert!(!file.exists());
}

#[test]
fn a_file_the_restored_database_lists_is_kept_whoever_else_names_it() {
    let (fake, file) = extra_track_setup(true, ":iPod_Control:Music:F00:TRK001.mp3");

    restore_backup(&opened(&fake)).unwrap();

    assert!(file.exists());
}

#[test]
fn a_cover_changed_by_the_undone_sync_is_looked_at_again_and_one_it_left_alone_is_not() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let red = covered_track(library.path(), 1, "One", [255, 0, 0]);
    let green = covered_track(library.path(), 2, "Two", [0, 255, 0]);
    sync(&fake, &[red.clone(), green.clone()], &[]);
    let (key_one, key_two) = (artwork_key(&fake, 1), artwork_key(&fake, 2));
    assert!(key_one.is_some() && key_two.is_some());
    let blue = covered_track(library.path(), 1, "One", [0, 0, 255]);
    sync(&fake, &[blue, green.clone()], &[]);
    assert_ne!(artwork_key(&fake, 1), key_one);

    restore_backup(&opened(&fake)).unwrap();

    assert_eq!(artwork_key(&fake, 2), key_two, "its cover never changed");
    assert_eq!(
        artwork_key(&fake, 1),
        None,
        "it points at the old image again"
    );
    let wanted = [red, green];
    assert_eq!(plan(&fake, &wanted, &[]).update, [1]);
}

fn covered_track(library: &Path, id: i64, title: &str, color: [u8; 3]) -> SyncTrack {
    let mut track = library_track(library, id, title);
    register_cover(&mut track, png(300, 300, color));
    track
}

#[test]
fn a_shuffle_gets_its_itunessd_written_again_from_the_restored_database() {
    let fake = FakeIpod::new("SHUFFLE");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("xB518", "0x000A270011223344");
    let library = tempfile::tempdir().unwrap();
    let one = library_track(library.path(), 1, "One");
    let two = library_track(library.path(), 2, "Two");
    sync(&fake, std::slice::from_ref(&one), &[]);
    let sd_after_first = fs::read(itunes_file(&fake, "iTunesSD")).unwrap();
    sync(&fake, &[one, two], &[]);
    assert_eq!(
        parse_fixed(&fs::read(itunes_file(&fake, "iTunesSD")).unwrap())
            .tracks
            .len(),
        7
    );

    restore_backup(&opened(&fake)).unwrap();

    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesSD")).unwrap(),
        sd_after_first
    );
}

#[test]
fn nothing_is_restored_without_a_backup() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);
    fs::remove_file(backup_file(&fake)).unwrap();

    assert_refused_untouched(&fake, is_refusal);
}

#[test]
fn a_backup_that_does_not_parse_is_refused_untouched() {
    let state = two_syncs();
    let backup = fs::read(backup_file(&state.fake)).unwrap();
    for broken in [
        b"not a database".to_vec(),
        Vec::new(),
        backup[..backup.len() / 2].to_vec(),
    ] {
        fs::write(backup_file(&state.fake), broken).unwrap();

        assert_refused_untouched(&state.fake, is_refusal);
    }
}

#[test]
fn a_backup_with_the_wrong_checksum_is_refused_untouched() {
    let state = two_syncs();
    let mut backup = fs::read(backup_file(&state.fake)).unwrap();
    backup[0x58] ^= 0xff;
    fs::write(backup_file(&state.fake), backup).unwrap();

    assert_refused_untouched(&state.fake, is_refusal);
}

#[test]
fn the_unhashed_original_cannot_be_put_back_on_a_device_that_needs_a_checksum() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);

    assert_refused_untouched(&fake, is_refusal);
}

#[test]
fn play_counts_and_on_the_go_files_must_be_dealt_with_first() {
    let state = two_syncs();
    let counts = itunes_file(&state.fake, "Play Counts");
    fs::write(&counts, play_counts_file(&[])).unwrap();
    assert_refused_untouched(&state.fake, |error| matches!(error, Error::Conflict(_)));
    fs::remove_file(counts).unwrap();

    fs::write(itunes_file(&state.fake, "OTGPlaylistInfo"), b"mhpo").unwrap();
    assert_refused_untouched(&state.fake, |error| matches!(error, Error::Conflict(_)));
}

#[test]
fn music_the_undone_sync_deleted_cannot_be_listed_again() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let one = library_track(library.path(), 1, "One");
    let two = library_track(library.path(), 2, "Two");
    sync(&fake, &[one.clone(), two.clone()], &[]);
    let removed = track_path(&fake, "One");
    sync(&fake, &[two], &[]);
    assert!(!removed.exists());

    assert_refused_untouched(&fake, is_refusal);
}

#[test]
fn a_device_that_cannot_be_written_is_refused() {
    let fake = FakeIpod::new("NANO6");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("xC526", "0x000A270011223344");
    fake.write_sysinfo_extended("000A270011223344", "7T0000000ABC", 5);
    fs::write(backup_file(&fake), sample_database().build()).unwrap();

    let result = restore_backup(&opened(&fake));

    assert!(matches!(result, Err(Error::SyncRefused(_))), "{result:?}");
}

#[test]
fn a_failure_staging_a_file_changes_nothing() {
    for blocked in ["iTunesDB.unsub-tmp", "Unsub.json.unsub-tmp"] {
        let state = two_syncs();
        fs::create_dir(itunes_file(&state.fake, blocked)).unwrap();
        let database = db_bytes(&state.fake);
        let mapping = fs::read(itunes_file(&state.fake, "Unsub.json")).unwrap();
        let music = music_files(&state.fake);
        let listing = itunes_dir_listing(&state.fake);

        let result = restore_backup(&opened(&state.fake));

        assert!(
            matches!(result, Err(Error::Io { .. })),
            "{blocked}: {result:?}"
        );
        assert_eq!(db_bytes(&state.fake), database, "{blocked}");
        assert_eq!(
            fs::read(itunes_file(&state.fake, "Unsub.json")).unwrap(),
            mapping
        );
        assert_eq!(music_files(&state.fake), music, "{blocked}");
        assert_eq!(
            itunes_dir_listing(&state.fake),
            listing,
            "{blocked}: no staged file may be left behind"
        );
    }
}
