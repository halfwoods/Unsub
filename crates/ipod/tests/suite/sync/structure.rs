use std::fs;

use unsub_ipod::{SyncPlaylist, read_database};

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
        fs::read(itunes_file(&fake, "iTunesDB.unsub-backup")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesDB.unsub-original")).unwrap(),
        original
    );

    sync(
        &fake,
        &[tracks[0].clone(), library_track(library.path(), 2, "Two")],
        &[],
    );
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesDB.unsub-backup")).unwrap(),
        first
    );
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesDB.unsub-original")).unwrap(),
        original,
        "the pre-sync original is never overwritten"
    );
}

#[test]
fn no_temporary_files_remain_after_a_successful_sync() {
    let (fake, _library) = synced_library();

    let listing = itunes_dir_listing(&fake);

    assert!(
        listing.iter().all(|name| !name.ends_with(".unsub-tmp")),
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

fn album_offset(bytes: &[u8], album_id: u32) -> usize {
    let section = chunks(bytes, b"mhsd", 0x60)
        .into_iter()
        .find(|&at| u32_at(bytes, at + 12) == 4)
        .unwrap();
    let list = section + u32_at(bytes, section + 4) as usize;
    let mut at = list + u32_at(bytes, list + 4) as usize;
    for _ in 0..u32_at(bytes, list + 8) {
        assert_eq!(&bytes[at..at + 4], b"mhia");
        if u32_at(bytes, at + 16) == album_id {
            return at;
        }
        at += u32_at(bytes, at + 8) as usize;
    }
    panic!("missing album {album_id}")
}

fn album_record(fake: &FakeIpod, title: &str) -> Vec<u8> {
    let bytes = db_bytes(fake);
    let track = read_database(&opened(fake))
        .unwrap()
        .tracks
        .into_iter()
        .find(|track| track.title == title)
        .unwrap();
    let (header, _) = crate::common::db::track_record(&bytes, track.dbid).unwrap();
    let at = album_offset(&bytes, u32_at(&header, 0x120));
    bytes[at..at + u32_at(&bytes, at + 8) as usize].to_vec()
}

fn decorate_album(fake: &FakeIpod, title: &str, pid: u64, supplier: u64, marker: u8) {
    let mut bytes = db_bytes(fake);
    let record = album_record(fake, title);
    let album_id = u32_at(&record, 16);
    let at = album_offset(&bytes, album_id);
    let section = chunks(&bytes, b"mhsd", 0x60)
        .into_iter()
        .find(|&at| u32_at(&bytes, at + 12) == 4)
        .unwrap();
    let extra = crate::common::db::mhod_raw(0xf00d, &[marker, 0, 0xff, 7]);
    let end = at + record.len();
    bytes.splice(end..end, extra.iter().copied());
    bytes.splice(at + 88..at + 88, [marker; 8]);
    bytes[at + 4..at + 8].copy_from_slice(&96u32.to_le_bytes());
    bytes[at + 20..at + 28].copy_from_slice(&pid.to_le_bytes());
    bytes[at + 30] = 2;
    bytes[at + 32..at + 40].copy_from_slice(&supplier.to_le_bytes());
    bytes[at + 72] = marker;
    let children = u32_at(&bytes, at + 12) + 1;
    bytes[at + 12..at + 16].copy_from_slice(&children.to_le_bytes());
    bytes[at + 8..at + 12]
        .copy_from_slice(&((record.len() + extra.len() + 8) as u32).to_le_bytes());
    let section_len = u32_at(&bytes, section + 8) + extra.len() as u32 + 8;
    bytes[section + 8..section + 12].copy_from_slice(&section_len.to_le_bytes());
    let total = bytes.len() as u32;
    bytes[8..12].copy_from_slice(&total.to_le_bytes());
    fake.write_itunesdb(&bytes);
}

fn assert_album_metadata(record: &[u8], pid: u64, supplier: u64, marker: u8) {
    assert_eq!(u64_at(record, 20), pid, "original album persistent ID");
    assert_eq!(
        u64_at(record, 32),
        supplier,
        "original album artwork selector"
    );
    assert_eq!(record[30], 2, "original artwork status");
    assert_eq!(record[72], marker, "unknown album header byte");
    assert_eq!(u32_at(record, 4), 96, "original extended header length");
    assert_eq!(
        &record[88..96],
        &[marker; 8],
        "unknown extended header bytes"
    );
    assert!(
        record
            .windows(28)
            .any(|window| window == crate::common::db::mhod_raw(0xf00d, &[marker, 0, 0xff, 7])),
        "unknown album child"
    );
}

#[test]
fn music_and_video_adds_preserve_independent_album_metadata_for_the_same_title_by_different_artists()
 {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::AtomicBool;
    use unsub_ipod::video::VideoKind;
    use unsub_ipod::{IpodMediaKind, VideoSyncTrack, apply_video_additions};

    let mut spec = sample_database();
    spec.tracks.truncate(2);
    for (index, track) in spec.tracks.iter_mut().enumerate() {
        track.title = format!("Foreign {index}");
        track.album = "Shared title".into();
        track.album_artist = format!("Artist {index}");
        track.artist = track.album_artist.clone();
    }
    let fake = classic_7g(&spec);
    sync(&fake, &[], &[]);
    let supplier = read_database(&opened(&fake)).unwrap().tracks[1].dbid;
    decorate_album(&fake, "Foreign 0", 0x1234_5678, supplier, 0xa1);
    decorate_album(&fake, "Foreign 1", 0x8765_4321, 0xffff_eeee_dddd_cccc, 0xb2);
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &[library_track(library.path(), 7, "New music")], &[]);
    assert_album_metadata(
        &album_record(&fake, "Foreign 0"),
        0x1234_5678,
        supplier,
        0xa1,
    );
    assert_album_metadata(
        &album_record(&fake, "Foreign 1"),
        0x8765_4321,
        0xffff_eeee_dddd_cccc,
        0xb2,
    );

    let mut track = library_track(library.path(), 8, "New video");
    track.source_path = library.path().join("video.mp4");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../playback/tests/fixtures/video/baseline.mp4"),
        &track.source_path,
    )
    .unwrap();
    track.media_kind = IpodMediaKind::Video;
    let video = VideoSyncTrack {
        sha256: format!(
            "{:x}",
            Sha256::digest(fs::read(&track.source_path).unwrap())
        ),
        track,
        kind: VideoKind::Movie,
    };
    apply_video_additions(
        &opened(&fake),
        &[video],
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    assert_album_metadata(
        &album_record(&fake, "Foreign 0"),
        0x1234_5678,
        supplier,
        0xa1,
    );
    assert_album_metadata(
        &album_record(&fake, "Foreign 1"),
        0x8765_4321,
        0xffff_eeee_dddd_cccc,
        0xb2,
    );
}

#[test]
fn album_metadata_follows_the_unchanged_group_through_splits_merges_and_supplier_removal() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let mut one = library_track(library.path(), 7, "One");
    let mut two = library_track(library.path(), 8, "Two");
    sync(&fake, &[one.clone(), two.clone()], &[]);
    let supplier = read_database(&opened(&fake))
        .unwrap()
        .tracks
        .into_iter()
        .find(|track| track.title == "One")
        .unwrap()
        .dbid;
    decorate_album(&fake, "One", 0x1234_5678, supplier, 0xa1);

    one.album = "Split".into();
    sync(&fake, &[one.clone(), two.clone()], &[]);
    assert_album_metadata(&album_record(&fake, "Two"), 0x1234_5678, supplier, 0xa1);
    let split = album_record(&fake, "One");
    assert_ne!(u64_at(&split, 20), 0x1234_5678);
    assert_eq!(
        split[72], 0,
        "a renamed group must not inherit the old cover"
    );

    one.album = two.album.clone();
    sync(&fake, &[one.clone(), two.clone()], &[]);
    assert_album_metadata(&album_record(&fake, "One"), 0x1234_5678, supplier, 0xa1);
    sync(&fake, std::slice::from_ref(&two), &[]);
    let after_removal = album_record(&fake, "Two");
    assert_eq!(u64_at(&after_removal, 20), 0x1234_5678);
    assert_eq!(
        u64_at(&after_removal, 32),
        0,
        "a removed real supplier must not leave a dangling pointer"
    );
    assert_eq!(after_removal[30], 0);
    assert_eq!(after_removal[72], 0xa1);

    two.album = "Renamed entirely".into();
    sync(&fake, &[two], &[]);
    let renamed = album_record(&fake, "Two");
    assert_ne!(u64_at(&renamed, 20), 0x1234_5678);
    assert_eq!(renamed[72], 0);
}

#[test]
fn album_metadata_does_not_cross_tv_show_groups_with_the_same_artist_and_album() {
    let mut spec = sample_database();
    spec.tracks.truncate(2);
    for (index, track) in spec.tracks.iter_mut().enumerate() {
        track.title = format!("Episode {index}");
        track.album = "Season One".into();
        track.album_artist = "Studio".into();
        track.artist = "Studio".into();
        track.extra_mhods.push(crate::common::db::mhod_text(
            19,
            &format!("Show {index}"),
            crate::common::db::Encoding::Utf16,
        ));
    }
    let fake = classic_7g(&spec);
    sync(&fake, &[], &[]);
    decorate_album(&fake, "Episode 0", 0x1234_5678, 0xffff_0000, 0xa1);
    decorate_album(&fake, "Episode 1", 0x8765_4321, 0xeeee_0000, 0xb2);
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &[library_track(library.path(), 7, "New music")], &[]);
    assert_album_metadata(
        &album_record(&fake, "Episode 0"),
        0x1234_5678,
        0xffff_0000,
        0xa1,
    );
    assert_album_metadata(
        &album_record(&fake, "Episode 1"),
        0x8765_4321,
        0xeeee_0000,
        0xb2,
    );
}

#[test]
fn removing_the_first_album_member_keeps_metadata_for_the_artist_fallback_group() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let mut one = library_track(library.path(), 7, "First member");
    let mut two = library_track(library.path(), 8, "Remaining member");
    one.artist = "Artist".into();
    one.album_artist = "Album Artist".into();
    two.artist = "Artist".into();
    two.album_artist.clear();
    sync(&fake, &[one, two.clone()], &[]);
    let supplier = 0xffff_eeee_dddd_cccc;
    decorate_album(&fake, "First member", 0x1234_5678, supplier, 0xa1);
    sync(&fake, &[two], &[]);
    assert_album_metadata(
        &album_record(&fake, "Remaining member"),
        0x1234_5678,
        supplier,
        0xa1,
    );
}
