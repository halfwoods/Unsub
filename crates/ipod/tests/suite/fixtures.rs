use antistreaming_ipod::read_database;

use crate::common::mount::FakeIpod;
use crate::opened;

/// A database written by libgpod rather than by this crate's test builder, so the reader is checked
/// against an independent producer. See tests/fixtures/README.md.
#[test]
fn reads_a_database_written_by_libgpod() {
    let fake = FakeIpod::new("LIBGPOD");
    fake.write_itunesdb(include_bytes!("../fixtures/libgpod-empty-iTunesDB"));

    let ipod = opened(&fake);
    let database = read_database(&ipod).unwrap();

    assert_eq!(ipod.name, "Nicks iPod");
    assert!(database.tracks.is_empty());
    let playlists: Vec<_> = database
        .playlists
        .iter()
        .map(|playlist| {
            (
                playlist.id,
                playlist.name.as_str(),
                playlist.is_master,
                playlist.is_podcasts,
            )
        })
        .collect();
    assert_eq!(
        playlists,
        [
            (1_234_567_890, "Nicks iPod", true, false),
            (16_307_138_097_840_980_473, "Podcasts", false, true)
        ]
    );
}
