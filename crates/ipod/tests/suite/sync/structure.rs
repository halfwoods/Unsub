use std::fs;

use antistreaming_ipod::{SyncPlaylist, read_database};

use super::*;
use crate::common::db::{chunks, u32_at, u64_at};

fn synced_library() -> (FakeIpod, tempfile::TempDir) {
    let mut spec = sample_database();
    spec.artist_section = true;
    let fake = classic_7g(&spec);
    for folder in ["F01", "F02"] {
        fs::create_dir_all(fake.mount.join("iPod_Control/Music").join(folder)).unwrap();
    }
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 7, "Hunter"),
        library_track(library.path(), 8, "Bachelorette"),
    ];
    sync(
        &fake,
        &tracks,
        &[SyncPlaylist {
            name: "Mine".into(),
            tracks: vec![8, 7],
        }],
    );
    (fake, library)
}

#[test]
fn the_previous_database_is_kept_beside_the_new_one() {
    let fake = fake_classic();
    let original = db_bytes(&fake);
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 1, "One")];

    sync(&fake, &tracks, &[]);
    let first = db_bytes(&fake);
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesDB.antistreaming-backup")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesDB.antistreaming-original")).unwrap(),
        original
    );

    sync(
        &fake,
        &[tracks[0].clone(), library_track(library.path(), 2, "Two")],
        &[],
    );
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesDB.antistreaming-backup")).unwrap(),
        first
    );
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesDB.antistreaming-original")).unwrap(),
        original,
        "the pre-sync original is never overwritten"
    );
}

#[test]
fn no_temporary_files_remain_after_a_successful_sync() {
    let (fake, _library) = synced_library();

    let listing = itunes_dir_listing(&fake);

    assert!(
        listing
            .iter()
            .all(|name| !name.ends_with(".antistreaming-tmp")),
        "{listing:?}"
    );
}

#[test]
fn the_album_and_artist_lists_are_rebuilt_and_tracks_point_into_them() {
    let (fake, _library) = synced_library();
    let bytes = db_bytes(&fake);

    // Albums: Dangerously in Love, async, Some Show, Homogenic. Artists: Beyoncé, 坂本龍一, Björk.
    assert_eq!(chunks(&bytes, b"mhia", 88).len(), 4);
    assert_eq!(chunks(&bytes, b"mhii", 80).len(), 3);
    let album_ids: Vec<u32> = chunks(&bytes, b"mhit", 0x184)
        .into_iter()
        .map(|at| u32_at(&bytes, at + 0x120))
        .collect();
    assert_eq!(album_ids.len(), 7);
    assert!(album_ids.iter().all(|&id| id <= 4));
    let hunter = album_ids[5];
    assert_eq!(hunter, album_ids[6], "two tracks of one album share its id");
    assert_ne!(hunter, 0);
    assert_eq!(album_ids[3], 0, "a track without an album has none");
}

#[test]
fn the_master_playlist_carries_five_sort_indexes_and_five_jump_tables_in_each_playlist_section() {
    let (fake, _library) = synced_library();
    let bytes = db_bytes(&fake);

    let mhods = chunks(&bytes, b"mhod", 24);
    let of_type = |kind: u32| {
        mhods
            .iter()
            .filter(|&&at| u32_at(&bytes, at + 12) == kind)
            .count()
    };

    // The master playlist appears in both the type 2 and the type 3 section.
    assert_eq!(of_type(52), 10);
    assert_eq!(of_type(53), 10);
}

#[test]
fn the_title_index_lists_every_master_member_in_title_order() {
    let (fake, _library) = synced_library();
    let bytes = db_bytes(&fake);
    let read = read_database(&opened(&fake)).unwrap();
    let master = read.playlists.iter().find(|p| p.is_master).unwrap();
    let title_index = chunks(&bytes, b"mhod", 24)
        .into_iter()
        .find(|&at| u32_at(&bytes, at + 12) == 52 && u32_at(&bytes, at + 24) == 3)
        .unwrap();

    let count = u32_at(&bytes, title_index + 28) as usize;
    let order: Vec<usize> = (0..count)
        .map(|i| u32_at(&bytes, title_index + 72 + 4 * i) as usize)
        .collect();

    assert_eq!(count, master.track_ids.len());
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        (0..count).collect::<Vec<_>>(),
        "a permutation of the member positions"
    );
    let key = |position: usize| {
        let track = read
            .tracks
            .iter()
            .find(|t| t.id == master.track_ids[position])
            .unwrap();
        track
            .sort_title
            .clone()
            .unwrap_or_else(|| track.title.clone())
            .to_lowercase()
    };
    let keys: Vec<String> = order.iter().map(|&position| key(position)).collect();
    assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]), "{keys:?}");
}

#[test]
fn sections_this_crate_does_not_derive_are_copied_untouched() {
    let fake = fake_classic();
    let before = db_bytes(&fake);
    let library = tempfile::tempdir().unwrap();

    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);

    let after = db_bytes(&fake);
    let section = |bytes: &[u8], kind: u32| {
        let at = chunks(bytes, b"mhsd", 0x60)
            .into_iter()
            .find(|&at| u32_at(bytes, at + 12) == kind)
            .unwrap();
        bytes[at..at + u32_at(bytes, at + 8) as usize].to_vec()
    };
    assert_eq!(section(&after, 5), section(&before, 5));
}

#[test]
fn chunk_lengths_are_consistent_throughout_the_written_file() {
    let (fake, _library) = synced_library();
    let bytes = db_bytes(&fake);

    assert_eq!(u32_at(&bytes, 8) as usize, bytes.len());
    // The mhsd sections tile the file exactly after the mhbd header.
    let mut at = u32_at(&bytes, 4) as usize;
    let mut sections = 0;
    while at < bytes.len() {
        assert_eq!(&bytes[at..at + 4], b"mhsd", "offset {at}");
        at += u32_at(&bytes, at + 8) as usize;
        sections += 1;
    }
    assert_eq!(at, bytes.len());
    assert_eq!(sections as u32, u32_at(&bytes, 20));
}

#[test]
fn a_sync_with_no_changes_leaves_what_the_reader_sees_identical() {
    let fake = fake_classic();
    let before = read_database(&opened(&fake)).unwrap();

    sync(&fake, &[], &[]);

    assert_eq!(read_database(&opened(&fake)).unwrap(), before);
}

#[test]
fn hash58_devices_get_a_checksum_and_older_ipods_are_left_without_one() {
    let (fake, _library) = synced_library();
    let bytes = db_bytes(&fake);
    assert_eq!(&bytes[0x30..0x32], &[1, 0], "scheme 1 is HASH58");
    assert_ne!(&bytes[0x58..0x6c], &[0; 20], "the checksum is filled in");
    assert_eq!(
        u64_at(&bytes, 0x18),
        0x1122_3344_5566_7788,
        "the database id survives sealing"
    );

    let old = FakeIpod::new("OLDPOD");
    old.write_itunesdb(&sample_database().build());
    old.write_sysinfo("x8541", "0x000A27001A2B3C4D");
    let library = tempfile::tempdir().unwrap();
    sync(&old, &[library_track(library.path(), 1, "One")], &[]);
    let bytes = db_bytes(&old);
    assert_eq!(&bytes[0x30..0x32], &[0, 0]);
    assert_eq!(&bytes[0x58..0x6c], &[0; 20]);
}
