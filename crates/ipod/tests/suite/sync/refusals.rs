use antistreaming_ipod::{Error, apply_sync, clear_play_counts, plan_sync, read_database};

use super::*;

fn assert_refused(fake: &FakeIpod, reason_contains: &str) {
    let ipod = opened(fake);
    let current = read_database(&ipod).unwrap();
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 1, "One")];
    let database = db_bytes(fake);
    let dir = itunes_dir_listing(fake);
    let music = music_files(fake);

    let planned = plan_sync(&ipod, &current, &tracks, &[]);
    let forged = SyncPlan {
        add: vec![1],
        update: vec![],
        remove: vec![],
        bytes_needed: 4000,
        fits: true,
    };
    let applied = apply_sync(&ipod, &forged, &tracks, &[], &mut copy_as_is, &mut |_| {});
    let cleared = clear_play_counts(&ipod);

    for result in [planned.map(drop), applied.map(drop), cleared] {
        match result {
            Err(Error::SyncRefused(reason)) => {
                assert!(reason.contains(reason_contains), "{reason}")
            }
            other => panic!("expected a refusal mentioning {reason_contains:?}, got {other:?}"),
        }
    }
    assert_eq!(db_bytes(fake), database);
    assert_eq!(itunes_dir_listing(fake), dir);
    assert_eq!(music_files(fake), music);
}

#[test]
fn a_nano_6g_needing_a_hashab_checksum_is_refused() {
    let fake = FakeIpod::new("NANO6");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("xC526", "0x000A270011223344");
    fake.write_sysinfo_extended("000A270011223344", "7T0000000ABC", 5);

    assert_refused(&fake, "HASHAB");
}

#[test]
fn a_hash72_device_is_refused() {
    let fake = FakeIpod::new("NANO5");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("xC060", "0x000A270011223344");
    fake.write_sysinfo_extended("000A270011223344", "7T0000000ABC", 4);

    assert_refused(&fake, "HASH72");
}

#[test]
fn an_unidentified_device_is_refused() {
    let fake = FakeIpod::new("MYSTERY");
    fake.write_itunesdb(&sample_database().build());

    assert_refused(&fake, "unknown");
}

#[test]
fn a_device_with_an_itunescdb_library_is_refused() {
    let fake = FakeIpod::new("NANO");
    fake.write_itunescdb(&sample_database().build_cdb());
    fake.write_sysinfo("xB598", "0x000A270011223344");

    assert_refused(&fake, "iTunesCDB");
}

#[test]
fn a_shuffle_is_refused() {
    let fake = FakeIpod::new("SHUFFLE");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("xB518", "0x000A270011223344");

    assert_refused(&fake, "iTunesSD");
}

#[test]
fn a_hash58_device_without_a_firewire_guid_is_refused() {
    let fake = FakeIpod::new("CLASSIC");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("xC293", "");

    assert_refused(&fake, "GUID");
}

#[test]
fn a_damaged_bookkeeping_file_is_an_error_not_a_guess() {
    let fake = fake_classic();
    fs::write(itunes_file(&fake, "AntiStreaming.json"), b"{ not json").unwrap();
    let ipod = opened(&fake);
    let current = read_database(&ipod).unwrap();

    let result = plan_sync(&ipod, &current, &[], &[]);

    assert!(matches!(result, Err(Error::Mapping(_))), "{result:?}");
}

#[test]
fn a_database_the_writer_cannot_reproduce_is_refused_untouched() {
    let fake = fake_classic();
    let before = db_bytes(&fake);
    // Truncated: the reader already rejects it, and so must a sync.
    fake.write_itunesdb(&before[..before.len() / 2]);
    let truncated = db_bytes(&fake);
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 1, "One")];
    let plan = SyncPlan {
        add: vec![1],
        update: vec![],
        remove: vec![],
        bytes_needed: 4000,
        fits: true,
    };

    let result = apply_sync(
        &opened(&fake),
        &plan,
        &tracks,
        &[],
        &mut copy_as_is,
        &mut |_| {},
    );

    assert!(matches!(result, Err(Error::Corrupt { .. })), "{result:?}");
    assert_eq!(db_bytes(&fake), truncated);
    assert!(music_files(&fake).is_empty());
}
