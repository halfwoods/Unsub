use antistreaming_ipod::{IpodPlaylist, read_database};

use crate::common::db::{Database, Playlist};
use crate::common::mount::FakeIpod;
use crate::{classic_7g, opened, sample_database, sample_playlists, sample_tracks};

fn read_playlists(database: &Database) -> Vec<IpodPlaylist> {
    let fake = classic_7g(database);
    read_database(&opened(&fake)).unwrap().playlists
}

fn named<'a>(playlists: &'a [IpodPlaylist], name: &str) -> &'a IpodPlaylist {
    playlists
        .iter()
        .find(|playlist| playlist.name == name)
        .unwrap_or_else(|| panic!("no playlist {name}"))
}

#[test]
fn reads_master_normal_podcast_and_smart_playlists() {
    let playlists = read_playlists(&sample_database());

    let names: Vec<_> = playlists
        .iter()
        .map(|playlist| playlist.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["Riley’s iPod", "Road Trip", "Top Rated", "Podcasts"]
    );

    let master = named(&playlists, "Riley’s iPod");
    assert!(master.is_master && !master.is_podcasts && !master.is_smart);
    assert_eq!(master.id, 0x1000);
    assert_eq!(master.track_ids, [1, 2, 3, 4, 5]);

    let road_trip = named(&playlists, "Road Trip");
    assert!(!road_trip.is_master && !road_trip.is_podcasts && !road_trip.is_smart);

    let top_rated = named(&playlists, "Top Rated");
    assert!(top_rated.is_smart && !top_rated.is_master);
    assert_eq!(top_rated.track_ids, [1]);

    let podcasts = named(&playlists, "Podcasts");
    assert!(podcasts.is_podcasts && !podcasts.is_master);
}

#[test]
fn members_follow_the_position_mhod_not_file_order() {
    let playlists = read_playlists(&sample_database());

    assert_eq!(named(&playlists, "Road Trip").track_ids, [1, 5, 2]);
}

#[test]
fn podcast_group_headers_are_not_members() {
    let playlists = read_playlists(&sample_database());

    assert_eq!(named(&playlists, "Podcasts").track_ids, [3]);
}

#[test]
fn plain_playlist_section_is_used_when_there_is_no_podcast_section() {
    let mut database = sample_database();
    database.podcast_section = false;

    let playlists = read_playlists(&database);

    assert_eq!(named(&playlists, "Podcasts").track_ids, [3]);
    assert_eq!(playlists.len(), 4);
}

#[test]
fn smart_playlist_and_album_sections_are_ignored() {
    let playlists = read_playlists(&sample_database());

    assert!(
        playlists
            .iter()
            .all(|playlist| playlist.name != "IGNORED-TYPE-5")
    );
}

#[test]
fn reads_mhips_that_old_itunes_wrote_with_total_length_equal_to_header_length() {
    let mut playlists = sample_playlists();
    for playlist in &mut playlists {
        playlist.legacy_mhips = true;
    }
    let database = Database::new(sample_tracks(), playlists);

    let read = read_playlists(&database);

    assert_eq!(named(&read, "Road Trip").track_ids, [1, 5, 2]);
    assert_eq!(named(&read, "Riley’s iPod").track_ids, [1, 2, 3, 4, 5]);
    assert_eq!(read.len(), 4);
}

#[test]
fn a_database_with_no_playlists_at_all_is_an_empty_list() {
    let fake = FakeIpod::new("IPOD");
    fake.write_itunesdb(&Database::new(sample_tracks(), Vec::<Playlist>::new()).build());

    let database = read_database(&opened(&fake)).unwrap();

    assert!(database.playlists.is_empty());
    assert_eq!(database.tracks.len(), 5);
}

#[test]
fn playlist_ids_serialize_as_strings() {
    let playlists = read_playlists(&sample_database());

    let json = serde_json::to_value(&playlists[0]).unwrap();

    assert_eq!(json["id"], "4096");
    assert_eq!(json["isMaster"], true);
    assert_eq!(json["trackIds"], serde_json::json!([1, 2, 3, 4, 5]));
}
