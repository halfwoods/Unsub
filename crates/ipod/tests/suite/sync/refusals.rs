use unsub_ipod::{Error, apply_sync, clear_play_counts, plan_sync, read_database};

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
        removals_first: false,
    };
    let applied = apply_sync(
        &ipod,
        &forged,
        &tracks,
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    );
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
fn an_unrecognised_device_with_an_itunessd_is_refused() {
    // Its song list lives in a file this crate would leave stale, so removed music would still be
    // listed by it.
    let fake = FakeIpod::new("MYSTERY");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo_extended("000A270011223344", "7T0000000ABC", 0);
    fs::write(itunes_file(&fake, "iTunesSD"), b"whatever the device wrote").unwrap();

    assert_refused(&fake, "iTunesSD");
}

#[test]
fn a_shuffle_whose_itunessd_format_is_unknown_is_refused() {
    let fake = FakeIpod::new("SHUFFLE");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("xB518", "0x000A270011223344");
    fake.write_shadow_db_version(9);

    assert_refused(&fake, "iTunesSD format");
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
    fs::write(itunes_file(&fake, "Unsub.json"), b"{ not json").unwrap();
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
        removals_first: false,
    };

    let result = apply_sync(
        &opened(&fake),
        &plan,
        &tracks,
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    );

    assert!(matches!(result, Err(Error::Corrupt { .. })), "{result:?}");
    assert_eq!(db_bytes(&fake), truncated);
    assert!(music_files(&fake).is_empty());
}

/// A classic that a sync has written a HASH58 database to with the GUID in its SysInfo, and the
/// same iPod as it is opened when that GUID is not in its files and came from the USB connection.
fn synced_classic_read_over_usb(guid: &str) -> (FakeIpod, unsub_ipod::Ipod) {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);
    let mut ipod = opened(&fake);
    ipod.firewire_guid = Some(guid.to_owned());
    ipod.guid_from_usb = true;
    (fake, ipod)
}

fn plan_and_apply_one_track(
    ipod: &unsub_ipod::Ipod,
) -> (
    Result<unsub_ipod::SyncPlan, Error>,
    Result<unsub_ipod::Database, Error>,
) {
    let current = read_database(ipod).unwrap();
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 2, "Two")];
    let planned = plan_sync(ipod, &current, &tracks, &[]);
    let forged = SyncPlan {
        add: vec![2],
        update: vec![],
        remove: vec![],
        bytes_needed: 4000,
        fits: true,
        removals_first: false,
    };
    let applied = apply_sync(
        ipod,
        &forged,
        &tracks,
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    );
    (planned, applied)
}

#[test]
fn a_guid_from_usb_that_does_not_match_the_databases_checksum_is_refused_untouched() {
    let (fake, ipod) = synced_classic_read_over_usb("000A27009999AAAA");
    let before = (
        db_bytes(&fake),
        music_files(&fake),
        itunes_dir_listing(&fake),
    );

    let (planned, applied) = plan_and_apply_one_track(&ipod);

    for result in [planned.map(drop), applied.map(drop)] {
        match result {
            Err(Error::SyncRefused(reason)) => assert!(reason.contains("USB"), "{reason}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
    assert_eq!(
        (
            db_bytes(&fake),
            music_files(&fake),
            itunes_dir_listing(&fake)
        ),
        before
    );
}

#[test]
fn a_guid_from_usb_that_matches_the_databases_checksum_syncs() {
    let (_fake, ipod) = synced_classic_read_over_usb("000A27001A2B3C4D");

    let (planned, applied) = plan_and_apply_one_track(&ipod);

    planned.unwrap();
    applied.unwrap();
}

#[test]
fn a_guid_from_the_ipods_own_files_is_not_second_guessed() {
    // The same wrong GUID, but SysInfo says it: whatever the database says, that is the iPod's word.
    let (_fake, mut ipod) = synced_classic_read_over_usb("000A27009999AAAA");
    ipod.guid_from_usb = false;

    let (planned, _) = plan_and_apply_one_track(&ipod);

    planned.unwrap();
}
