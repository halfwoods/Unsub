use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use unsub_ipod::{ArtworkCheck, ChecksumCheck, Error, verify_roundtrip};

use crate::common::artwork::png;
use crate::common::db::{Database as DbSpec, Track, chunks, u32_at};
use crate::common::mount::FakeIpod;
use crate::sync::artwork::{art_dir, classic_with_foreign_artwork, covered};
use crate::sync::{db_bytes, fake_classic, library_track, sync};
use crate::{classic_7g, opened, sample_database, sample_playlists, sample_tracks};

fn tree(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)> {
    fn walk(dir: &Path, found: &mut BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else {
                let modified = fs::metadata(&path).unwrap().modified().unwrap();
                found.insert(path.clone(), (fs::read(&path).unwrap(), modified));
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(root, &mut found);
    found
}

#[test]
fn a_synthetic_database_survives_a_rewrite_losslessly() {
    let fake = classic_7g(&sample_database());

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    assert!(report.is_lossless(), "{report:#?}");
    assert_eq!((report.tracks, report.playlists), (5, 4));
    assert!(report.track_differences.is_empty() && report.playlist_differences.is_empty());
    assert!(report.mhod_differences.is_empty());
    assert_eq!(report.checksum, ChecksumCheck::Absent);
    // Derived data appears: the album list (the fixture's was empty) and, in both playlist
    // sections, the master playlist's sort indexes.
    let counts: Vec<_> = report
        .chunk_counts
        .iter()
        .map(|c| (c.chunk.as_str(), c.original, c.rewritten))
        .collect();
    assert_eq!(
        counts,
        [
            ("mhia", 0, 3),
            ("mhod:200", 0, 3),
            ("mhod:201", 0, 2),
            ("mhod:202", 0, 1),
            ("mhod:52", 0, 10),
            ("mhod:53", 0, 10)
        ]
    );
}

#[test]
fn the_report_lists_every_section_with_its_size_change() {
    let fake = classic_7g(&sample_database());

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    let kinds: Vec<_> = report
        .sections
        .iter()
        .map(|s| (s.kind, s.rebuilt))
        .collect();
    assert_eq!(
        kinds,
        [(1, true), (3, true), (2, true), (4, true), (5, false)]
    );
    let smart = report.sections.iter().find(|s| s.kind == 5).unwrap();
    assert_eq!(
        smart.original_bytes, smart.rewritten_bytes,
        "copied sections are unchanged"
    );
    let playlists = report.sections.iter().find(|s| s.kind == 2).unwrap();
    assert!(
        playlists.rewritten_bytes > playlists.original_bytes,
        "the indexes add bytes"
    );
    assert_eq!(
        report.rewritten_bytes as i64 - report.original_bytes as i64,
        report
            .sections
            .iter()
            .map(|s| s.rewritten_bytes as i64 - s.original_bytes as i64)
            .sum::<i64>(),
    );
}

#[test]
fn a_database_written_by_libgpod_survives_a_rewrite_losslessly() {
    let fake = FakeIpod::new("LIBGPOD");
    fake.write_itunesdb(include_bytes!("../fixtures/libgpod-empty-iTunesDB"));

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    assert!(report.is_lossless(), "{report:#?}");
    assert_eq!((report.tracks, report.playlists), (0, 2));
}

#[test]
fn old_style_and_unmodelled_header_fields_are_reproduced_exactly() {
    let mut tracks = sample_tracks();
    tracks[0].header_patches = vec![
        (184, 2112u32.to_le_bytes().to_vec()),
        (248, 7u32.to_le_bytes().to_vec()),
    ];
    let mut spec = DbSpec::new(tracks, sample_playlists());
    spec.podcast_section = false;
    let fake = FakeIpod::new("POD");
    fake.write_itunesdb(&spec.build());

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    assert!(report.is_lossless(), "{report:#?}");
}

#[test]
fn playlist_members_that_point_at_missing_tracks_are_reported_as_dropped() {
    // Playlists name tracks 2, 3 and 5 but only track 1 exists: a damaged library iTunes tolerates.
    let spec = DbSpec::new(vec![Track::new(1, "Only")], sample_playlists());
    let fake = FakeIpod::new("DANGLING");
    fake.write_itunesdb(&spec.build());

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    assert!(!report.is_lossless());
    let road_trip = report
        .playlist_differences
        .iter()
        .find(|d| d.name == "Road Trip")
        .unwrap();
    assert_eq!(road_trip.fields.len(), 1);
    assert_eq!(road_trip.fields[0].field, "trackIds");
    assert_eq!(road_trip.fields[0].before, "[1,5,2]");
    assert_eq!(road_trip.fields[0].after, "[1]");
}

#[test]
fn a_chunk_the_writer_would_drop_is_reported() {
    let fake = FakeIpod::new("ODD");
    let mut bytes = sample_database().build();
    let tracks_section = chunks(&bytes, b"mhsd", 0x60)
        .into_iter()
        .find(|&at| u32_at(&bytes, at + 12) == 1)
        .unwrap();
    let mut unknown = b"zzzz".to_vec();
    unknown.extend_from_slice(&12u32.to_le_bytes());
    unknown.extend_from_slice(&16u32.to_le_bytes());
    unknown.extend_from_slice(&[0; 4]);
    let at = tracks_section + 0x60;
    bytes.splice(at..at, unknown);
    for length_at in [tracks_section + 8, 8] {
        let grown = u32_at(&bytes, length_at) + 16;
        bytes[length_at..length_at + 4].copy_from_slice(&grown.to_le_bytes());
    }
    fake.write_itunesdb(&bytes);

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    assert!(!report.is_lossless());
    let dropped = report
        .chunk_counts
        .iter()
        .find(|c| c.chunk == "unexpected:zzzz")
        .unwrap();
    assert_eq!((dropped.original, dropped.rewritten), (1, 0));
    let section = report.sections.iter().find(|s| s.kind == 1).unwrap();
    assert_eq!(section.original_bytes - section.rewritten_bytes, 16);
}

#[test]
fn a_checksum_written_by_a_sync_verifies_and_a_damaged_one_does_not() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);

    let report = verify_roundtrip(&opened(&fake)).unwrap();
    assert_eq!(report.checksum, ChecksumCheck::Verified);
    assert!(report.is_lossless(), "{report:#?}");

    // Flip a byte inside a track header that the reader ignores; the file still parses.
    let mut bytes = db_bytes(&fake);
    let track = chunks(&bytes, b"mhit", 0x184)[0];
    bytes[track + 0x100] ^= 0xff;
    fake.write_itunesdb(&bytes);
    let report = verify_roundtrip(&opened(&fake)).unwrap();
    assert_eq!(report.checksum, ChecksumCheck::Mismatch);
    assert!(!report.is_lossless());
}

#[test]
fn a_hash58_database_on_an_ipod_with_an_unknown_guid_says_so() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);
    fs::remove_file(fake.mount.join("iPod_Control/Device/SysInfoExtended")).unwrap();
    fake.write_sysinfo("xC293", "");

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    assert_eq!(report.checksum, ChecksumCheck::NoFirewireGuid);
}

#[test]
fn verification_never_changes_anything_on_the_device() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &[library_track(library.path(), 1, "One")], &[]);
    let before = tree(&fake.mount);

    verify_roundtrip(&opened(&fake)).unwrap();

    assert_eq!(
        tree(&fake.mount),
        before,
        "contents, modification times and file set are all untouched"
    );
}

#[test]
fn a_database_that_cannot_be_rewritten_is_an_error() {
    let fake = FakeIpod::new("BAD");
    let bytes = sample_database().build();
    fake.write_itunesdb(&bytes[..bytes.len() / 2]);

    let result = verify_roundtrip(&opened(&fake));

    assert!(matches!(result, Err(Error::Corrupt { .. })), "{result:?}");
}

#[test]
fn the_report_serialises_for_the_frontend() {
    let fake = classic_7g(&sample_database());

    let report = verify_roundtrip(&opened(&fake)).unwrap();
    let json = serde_json::to_value(&report).unwrap();

    assert_eq!(json["checksum"], "absent");
    assert_eq!(json["tracks"], 5);
    assert!(json["chunkCounts"].is_array());
}

#[test]
fn artwork_written_by_someone_else_is_read_and_reproduced_exactly() {
    let (fake, _) = classic_with_foreign_artwork();

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    assert_eq!(
        report.artwork,
        ArtworkCheck::Checked {
            images: 2,
            thumbnails: 8,
            missing_thumbnails: 0,
            identical: true
        }
    );
    assert!(report.is_lossless(), "{report:#?}");
}

#[test]
fn artwork_written_by_a_sync_verifies_too() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(
        &fake,
        &[covered(library.path(), 1, "One", png(300, 300, [1, 2, 3]))],
        &[],
    );

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    assert_eq!(
        report.artwork,
        ArtworkCheck::Checked {
            images: 1,
            thumbnails: 4,
            missing_thumbnails: 0,
            identical: true
        }
    );
    assert!(report.is_lossless(), "{report:#?}");
}

#[test]
fn thumbnails_that_point_outside_their_files_are_counted() {
    let (fake, _) = classic_with_foreign_artwork();
    let file = art_dir(&fake).join("F1060_1.ithmb");
    let bytes = fs::read(&file).unwrap();
    fs::write(&file, &bytes[..bytes.len() / 2]).unwrap();

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    assert_eq!(
        report.artwork,
        ArtworkCheck::Checked {
            images: 2,
            thumbnails: 8,
            missing_thumbnails: 1,
            identical: true
        }
    );
}

#[test]
fn an_artwork_database_this_crate_cannot_read_is_flagged() {
    let (fake, _) = classic_with_foreign_artwork();
    fs::write(
        art_dir(&fake).join("ArtworkDB"),
        b"garbage that is long enough to look like a file",
    )
    .unwrap();

    let report = verify_roundtrip(&opened(&fake)).unwrap();

    assert!(
        matches!(report.artwork, ArtworkCheck::Unreadable(_)),
        "{:?}",
        report.artwork
    );
    assert!(!report.is_lossless());
}
