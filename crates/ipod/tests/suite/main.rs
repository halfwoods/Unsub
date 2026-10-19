#[path = "../common/mod.rs"]
mod common;

mod cdb;
mod fixtures;
mod open;
mod play_counts;
mod playlists;
mod robustness;
mod tracks;

use antistreaming_ipod::{Ipod, open as open_ipod};
use chrono::{DateTime, Local, NaiveDate, TimeZone};
use common::db::{
    Database, Encoding, MEDIA_AUDIOBOOK, MEDIA_PODCAST, MEDIA_VIDEO, Playlist, Track,
};
use common::mount::FakeIpod;

const MAC_EPOCH_OFFSET: i64 = 2_082_844_800;

/// Mac-epoch seconds for a wall-clock moment, as iTunes writes it.
pub fn mac_wall_clock(year: i32, month: u32, day: u32, hour: u32) -> u32 {
    let wall_clock = NaiveDate::from_ymd_opt(year, month, day)
        .unwrap()
        .and_hms_opt(hour, 0, 0)
        .unwrap();
    u32::try_from(wall_clock.and_utc().timestamp() + MAC_EPOCH_OFFSET).unwrap()
}

/// The Unix time the reader should report for `mac_wall_clock`, resolved in the machine's zone.
pub fn unix_local(year: i32, month: u32, day: u32, hour: u32) -> i64 {
    let wall_clock = NaiveDate::from_ymd_opt(year, month, day)
        .unwrap()
        .and_hms_opt(hour, 0, 0)
        .unwrap();
    Local
        .from_local_datetime(&wall_clock)
        .earliest()
        .unwrap()
        .timestamp()
}

pub fn utc(secs: i64) -> DateTime<chrono::Utc> {
    DateTime::from_timestamp(secs, 0).unwrap()
}

/// Five tracks covering both string encodings and every media kind the capacity bar splits on.
pub fn sample_tracks() -> Vec<Track> {
    let mut crazy = Track::new(1, "Crazy in Love");
    crazy.artist = "Beyoncé".into();
    crazy.album_artist = "Beyoncé".into();
    crazy.album = "Dangerously in Love".into();
    crazy.genre = "R&B".into();
    crazy.composer = "Rich Harrison".into();
    crazy.sort_title = Some("Crazy in Love".into());
    crazy.sort_artist = Some("Beyonce".into());
    crazy.sort_album = Some("Dangerously in Love".into());
    crazy.sort_album_artist = Some("Beyonce".into());
    crazy.year = 2003;
    crazy.track_number = 2;
    crazy.track_count = 16;
    crazy.disc_number = 1;
    crazy.disc_count = 2;
    crazy.duration_ms = 236_000;
    crazy.bitrate = 320;
    crazy.sample_rate = 44_100;
    crazy.size = 9_440_000;
    crazy.play_count = 12;
    crazy.last_played_mac = mac_wall_clock(2010, 6, 15, 12);
    crazy.skip_count = 3;
    crazy.rating = 80;
    crazy.date_added_mac = mac_wall_clock(2009, 12, 1, 12);
    crazy.compilation = true;
    crazy.dbid = 0xfedc_ba98_7654_3210;

    let mut theme = Track::new(2, "戦場のメリークリスマス");
    theme.artist = "坂本龍一".into();
    theme.album = "async".into();
    theme.genre = "Electronic".into();
    theme.path = ":iPod_Control:Music:F07:ZXCV.m4a".into();
    theme.file_type = Some("AAC audio file".into());
    theme.encoding = Encoding::Utf8;
    theme.size = 7_000_000;

    let mut episode = Track::new(3, "Episode 42");
    episode.album = "Some Show".into();
    episode.media_type = MEDIA_PODCAST;
    episode.size = 30_000_000;
    episode.extra_mhods.push(common::db::mhod_raw(
        common::db::MHOD_PODCAST_URL,
        b"https://example.com/42.mp3",
    ));
    episode
        .extra_mhods
        .push(common::db::mhod_raw(99, b"unknown mhod types are skipped"));

    let mut book = Track::new(4, "Chapter 1");
    book.path = ":iPod_Control:Music:F03:BOOK.m4b".into();
    book.media_type = MEDIA_AUDIOBOOK;
    book.size = 50_000_000;

    let mut film = Track::new(5, "Trailer");
    film.path = ":iPod_Control:Music:F01:FILM.m4v".into();
    film.media_type = MEDIA_VIDEO;
    film.movie_flag = true;
    film.size = 100_000_000;

    vec![crazy, theme, episode, book, film]
}

pub fn sample_playlists() -> Vec<Playlist> {
    let mut road_trip = Playlist::new(0x1001, "Road Trip", &[]);
    // File order differs from playlist order: the reader must sort by position.
    road_trip.members = vec![(2, 2), (1, 0), (5, 1)];
    let mut top_rated = Playlist::new(0x1002, "Top Rated", &[1]);
    top_rated.smart = true;
    vec![
        Playlist::master(0x1000, "Riley’s iPod", &[1, 2, 3, 4, 5]),
        road_trip,
        top_rated,
        Playlist::podcasts(0x1003, "Podcasts", &[3]),
    ]
}

pub fn sample_database() -> Database {
    Database::new(sample_tracks(), sample_playlists())
}

/// An iPod classic 7G 160GB (silver) as iTunes left it: SysInfo, SysInfoExtended and an iTunesDB.
pub fn classic_7g(database: &Database) -> FakeIpod {
    let ipod = FakeIpod::new("RILEYS IPOD");
    ipod.write_itunesdb(&database.build());
    ipod.write_sysinfo("xC293", "0x000A27001A2B3C4D");
    ipod.write_sysinfo_extended("000A27001A2B3C4D", "9C9123456XYZ", 3);
    ipod
}

pub fn opened(fake: &FakeIpod) -> Ipod {
    open_ipod(&fake.mount).unwrap()
}
