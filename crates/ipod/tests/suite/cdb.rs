use unsub_ipod::{DatabaseFormat, Error, read_database};

use crate::common::mount::FakeIpod;
use crate::{opened, sample_database};

fn nano_with_cdb() -> FakeIpod {
    let fake = FakeIpod::new("NANO");
    fake.write_itunescdb(&sample_database().build_cdb());
    fake
}

#[test]
fn reads_a_compressed_database() {
    let fake = nano_with_cdb();

    let ipod = opened(&fake);
    let database = read_database(&ipod).unwrap();

    assert_eq!(ipod.database, DatabaseFormat::ITunesCdb);
    assert_eq!(ipod.name, "Riley’s iPod");
    assert_eq!(database.tracks.len(), 5);
    assert_eq!(database.tracks[0].title, "Crazy in Love");
    assert_eq!(database.playlists.len(), 4);
}

#[test]
fn compressed_and_plain_databases_parse_identically() {
    let plain = FakeIpod::new("PLAIN");
    plain.write_itunesdb(&sample_database().build());
    let compressed = nano_with_cdb();

    let from_plain = read_database(&opened(&plain)).unwrap();
    let from_compressed = read_database(&opened(&compressed)).unwrap();

    let strip_mount = |mut database: unsub_ipod::Database, mount: &std::path::Path| {
        for track in &mut database.tracks {
            track.path = track.path.strip_prefix(mount).unwrap().to_path_buf();
        }
        database
    };
    assert_eq!(
        strip_mount(from_plain, &plain.mount),
        strip_mount(from_compressed, &compressed.mount)
    );
}

#[test]
fn an_empty_itunesdb_yields_to_the_cdb() {
    let fake = nano_with_cdb();
    fake.write_itunesdb(b"");

    assert_eq!(opened(&fake).database, DatabaseFormat::ITunesCdb);
}

#[test]
fn the_cdb_wins_when_both_files_have_content() {
    let fake = nano_with_cdb();
    fake.write_itunesdb(&sample_database().build());

    assert_eq!(opened(&fake).database, DatabaseFormat::ITunesCdb);
}

#[test]
fn a_non_empty_itunesdb_wins_over_an_empty_cdb() {
    let fake = FakeIpod::new("IPOD");
    fake.write_itunesdb(&sample_database().build());
    fake.write_itunescdb(b"");

    assert_eq!(opened(&fake).database, DatabaseFormat::ITunesDb);
}

#[test]
fn a_corrupt_zlib_body_is_an_error_not_a_panic() {
    let fake = nano_with_cdb();
    let mut cdb = sample_database().build_cdb();
    let len = cdb.len();
    for byte in &mut cdb[0xbc + 4..len - 4] {
        *byte ^= 0x5a;
    }
    fake.write_itunescdb(&cdb);

    let error = read_database(&opened(&fake)).unwrap_err();

    assert!(matches!(error, Error::Corrupt { .. }), "{error}");
}

#[test]
fn a_header_too_short_for_the_compression_flag_is_corrupt() {
    let fake = nano_with_cdb();
    let mut cdb = sample_database().build_cdb();
    cdb[4..8].copy_from_slice(&0x68u32.to_le_bytes());
    fake.write_itunescdb(&cdb);

    let error = read_database(&opened(&fake)).unwrap_err();

    assert!(matches!(error, Error::Corrupt { .. }), "{error}");
}
