use antistreaming_ipod::{
    Error, OnTheGo, SyncPlan, apply_sync, clear_on_the_go, read_database, read_on_the_go,
};

use super::*;
use crate::common::db::on_the_go_file;

const ENTRY: usize = 4;
const HEADER: usize = 0x14;

fn write_otg(fake: &FakeIpod, name: &str, positions: &[u32]) {
    let file = on_the_go_file(positions, HEADER, ENTRY, false);
    fs::write(itunes_file(fake, name), file).unwrap();
}

fn on_the_go(fake: &FakeIpod) -> Result<Vec<OnTheGo>, Error> {
    let ipod = opened(fake);
    let database = read_database(&ipod).unwrap();
    read_on_the_go(&ipod, &database)
}

/// A classic with three synced tracks, at positions 5, 6 and 7 of the iTunesDB track list.
fn fake_with_synced_tracks() -> (FakeIpod, Vec<SyncTrack>, tempfile::TempDir) {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = vec![
        library_track(library.path(), 1, "Alpha"),
        library_track(library.path(), 2, "Bravo"),
        library_track(library.path(), 3, "Charlie"),
    ];
    sync(&fake, &tracks, &[]);
    (fake, tracks, library)
}

fn named(name: &str, tracks: &[i64]) -> OnTheGo {
    OnTheGo {
        name: name.to_owned(),
        tracks: tracks.to_vec(),
    }
}

#[test]
fn files_become_numbered_playlists_of_library_ids_without_the_tracks_the_app_did_not_add() {
    let (fake, ..) = fake_with_synced_tracks();
    // Position 0 is one of iTunes' own tracks, which the library does not have.
    write_otg(&fake, "OTGPlaylistInfo", &[6, 5, 0, 7]);
    write_otg(&fake, "OTGPlaylistInfo_1", &[7]);

    assert_eq!(
        on_the_go(&fake).unwrap(),
        [named("On-The-Go 1", &[2, 1, 3]), named("On-The-Go 2", &[3])]
    );
}

#[test]
fn a_playlist_of_nothing_but_foreign_tracks_is_left_out_but_keeps_its_number() {
    let (fake, ..) = fake_with_synced_tracks();
    write_otg(&fake, "OTGPlaylistInfo", &[0, 1]);
    write_otg(&fake, "OTGPlaylistInfo_1", &[5]);
    write_otg(&fake, "OTGPlaylistInfo_2", &[]);

    assert_eq!(on_the_go(&fake).unwrap(), [named("On-The-Go 2", &[1])]);
}

#[test]
fn no_files_means_no_playlists_and_so_do_files_when_nothing_was_synced() {
    let fake = fake_classic();
    assert_eq!(on_the_go(&fake).unwrap(), []);

    write_otg(&fake, "OTGPlaylistInfo", &[0, 1, 2]);
    assert_eq!(on_the_go(&fake).unwrap(), []);
}

#[test]
fn reading_stops_at_the_first_missing_file_in_the_sequence_as_libgpod_does() {
    let (fake, ..) = fake_with_synced_tracks();
    write_otg(&fake, "OTGPlaylistInfo", &[5]);
    write_otg(&fake, "OTGPlaylistInfo_2", &[6]);

    assert_eq!(on_the_go(&fake).unwrap(), [named("On-The-Go 1", &[1])]);
}

#[test]
fn file_names_are_matched_without_regard_to_case() {
    let (fake, ..) = fake_with_synced_tracks();
    write_otg(&fake, "otgplaylistinfo", &[5]);
    write_otg(&fake, "OTGPLAYLISTINFO_1", &[6]);

    assert_eq!(
        on_the_go(&fake).unwrap(),
        [named("On-The-Go 1", &[1]), named("On-The-Go 2", &[2])]
    );
}

#[test]
fn longer_headers_and_entries_and_phone_byte_order_are_read() {
    let (fake, ..) = fake_with_synced_tracks();
    let longer = on_the_go_file(&[7, 5], 0x40, 12, false);
    fs::write(itunes_file(&fake, "OTGPlaylistInfo"), longer).unwrap();
    let phone = on_the_go_file(&[6], HEADER, ENTRY, true);
    fs::write(itunes_file(&fake, "OTGPlaylistInfo_1"), phone).unwrap();

    assert_eq!(
        on_the_go(&fake).unwrap(),
        [named("On-The-Go 1", &[3, 1]), named("On-The-Go 2", &[2])]
    );
}

#[test]
fn a_file_that_does_not_fit_the_database_is_an_error_not_a_guess() {
    let (fake, ..) = fake_with_synced_tracks();
    let good = on_the_go_file(&[5, 6], HEADER, ENTRY, false);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "a position past the last track",
            on_the_go_file(&[5, 8], HEADER, ENTRY, false),
        ),
        ("no mhpo header", b"mhyp and then some more bytes".to_vec()),
        ("an empty file", Vec::new()),
        ("a short header", on_the_go_file(&[5], 0x10, ENTRY, false)),
        ("short entries", on_the_go_file(&[5], HEADER, 3, false)),
        ("entries cut off", good[..good.len() - 2].to_vec()),
        ("a count beyond the file", {
            let mut lying = good.clone();
            lying[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
            lying
        }),
    ];
    for (what, bytes) in cases {
        fs::write(itunes_file(&fake, "OTGPlaylistInfo"), bytes).unwrap();

        let result = on_the_go(&fake);

        assert!(
            matches!(result, Err(Error::Corrupt { .. })),
            "{what}: {result:?}"
        );
    }
}

#[test]
fn clearing_deletes_every_playlist_file_and_nothing_else() {
    let (fake, ..) = fake_with_synced_tracks();
    write_otg(&fake, "OTGPlaylistInfo", &[5]);
    write_otg(&fake, "OTGPlaylistInfo_1", &[6]);
    write_otg(&fake, "OTGPlaylistInfo_7", &[6]);
    for other in ["OTGPlaylistInfoX", "OTGPlaylistInfo_x", "Play Counts"] {
        fs::write(itunes_file(&fake, other), b"keep").unwrap();
    }
    let before = itunes_dir_listing(&fake);

    clear_on_the_go(&opened(&fake)).unwrap();

    let after = itunes_dir_listing(&fake);
    let removed: BTreeSet<_> = before.difference(&after).cloned().collect();
    assert_eq!(
        removed,
        BTreeSet::from_iter(
            ["OTGPlaylistInfo", "OTGPlaylistInfo_1", "OTGPlaylistInfo_7"].map(String::from)
        )
    );
    assert_eq!(after.difference(&before).count(), 0);
    assert_eq!(on_the_go(&fake).unwrap(), []);
    clear_on_the_go(&opened(&fake)).unwrap();
}

#[test]
fn clearing_is_refused_on_a_device_that_cannot_be_written() {
    let fake = FakeIpod::new("NANO6");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo("xC526", "0x000A270011223344");
    fake.write_sysinfo_extended("000A270011223344", "7T0000000ABC", 5);
    write_otg(&fake, "OTGPlaylistInfo", &[1]);

    let result = clear_on_the_go(&opened(&fake));

    assert!(matches!(result, Err(Error::SyncRefused(_))), "{result:?}");
    assert!(itunes_file(&fake, "OTGPlaylistInfo").exists());
}

#[test]
fn removing_tracks_is_refused_while_playlist_files_count_positions_in_the_list() {
    let (fake, tracks, _library) = fake_with_synced_tracks();
    write_otg(&fake, "OTGPlaylistInfo", &[6]);
    let database = db_bytes(&fake);
    let music = music_files(&fake);
    let keep = &tracks[1..];
    let plan = plan(&fake, keep, &[]);
    assert_eq!(plan.remove.len(), 1);

    let refused = apply_sync(
        &opened(&fake),
        &plan,
        keep,
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    );

    assert!(matches!(refused, Err(Error::Conflict(_))), "{refused:?}");
    assert_eq!(db_bytes(&fake), database);
    assert_eq!(music_files(&fake), music);

    clear_on_the_go(&opened(&fake)).unwrap();
    apply_sync(
        &opened(&fake),
        &plan,
        keep,
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(read_database(&opened(&fake)).unwrap().tracks.len(), 7);
}

#[test]
fn adding_tracks_is_fine_while_playlist_files_exist_as_positions_do_not_move() {
    let (fake, mut tracks, library) = fake_with_synced_tracks();
    write_otg(&fake, "OTGPlaylistInfo", &[6]);
    tracks.push(library_track(library.path(), 4, "Delta"));
    let plan: SyncPlan = plan(&fake, &tracks, &[]);
    assert_eq!(plan.add, [4]);

    sync(&fake, &tracks, &[]);

    assert_eq!(on_the_go(&fake).unwrap(), [named("On-The-Go 1", &[2])]);
}
