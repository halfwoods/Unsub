use std::path::PathBuf;

use antistreaming_ipod::{IpodMediaKind, IpodTrack, read_database};

use crate::common::db::{Database, Encoding, MEDIA_VIDEO, MHOD_PATH, Track, mhod_text};
use crate::common::mount::FakeIpod;
use crate::{classic_7g, opened, sample_database, unix_local};

fn read_sample() -> (FakeIpod, Vec<IpodTrack>) {
    let fake = classic_7g(&sample_database());
    let tracks = read_database(&opened(&fake)).unwrap().tracks;
    (fake, tracks)
}

fn read_tracks(tracks: Vec<Track>) -> (FakeIpod, Vec<IpodTrack>) {
    let fake = FakeIpod::new("IPOD");
    fake.write_itunesdb(&Database::new(tracks, crate::sample_playlists()).build());
    let tracks = read_database(&opened(&fake)).unwrap().tracks;
    (fake, tracks)
}

#[test]
fn reads_every_field_of_a_fully_populated_track() {
    let (fake, tracks) = read_sample();

    let track = &tracks[0];

    assert_eq!(tracks.len(), 5);
    assert_eq!(track.id, 1);
    assert_eq!(track.dbid, 0xfedc_ba98_7654_3210);
    assert_eq!(
        track.path,
        fake.mount.join("iPod_Control/Music/F00/TRK001.mp3")
    );
    assert_eq!(track.title, "Crazy in Love");
    assert_eq!(track.artist, "Beyoncé");
    assert_eq!(track.album_artist, "Beyoncé");
    assert_eq!(track.album, "Dangerously in Love");
    assert_eq!(track.genre, "R&B");
    assert_eq!(track.composer, "Rich Harrison");
    assert_eq!(track.year, Some(2003));
    assert_eq!(track.track_number, Some(2));
    assert_eq!(track.track_count, Some(16));
    assert_eq!(track.disc_number, Some(1));
    assert_eq!(track.disc_count, Some(2));
    assert_eq!(track.duration_ms, 236_000);
    assert_eq!(track.bitrate_kbps, Some(320));
    assert_eq!(track.sample_rate, Some(44_100));
    assert_eq!(track.file_size, 9_440_000);
    assert_eq!(track.play_count, 12);
    assert_eq!(track.last_played, Some(unix_local(2010, 6, 15, 12)));
    assert_eq!(track.skip_count, 3);
    assert_eq!(track.rating, 80);
    assert_eq!(track.date_added, Some(unix_local(2009, 12, 1, 12)));
    assert_eq!(track.media_kind, IpodMediaKind::Music);
    assert_eq!(track.file_type, "MPEG audio file");
    assert!(track.compilation);
    assert_eq!(track.sort_title.as_deref(), Some("Crazy in Love"));
    assert_eq!(track.sort_artist.as_deref(), Some("Beyonce"));
    assert_eq!(track.sort_album.as_deref(), Some("Dangerously in Love"));
    assert_eq!(track.sort_album_artist.as_deref(), Some("Beyonce"));
}

#[test]
fn reads_utf8_flagged_strings_and_non_latin_text() {
    let (fake, tracks) = read_sample();

    let track = &tracks[1];

    assert_eq!(track.title, "戦場のメリークリスマス");
    assert_eq!(track.artist, "坂本龍一");
    assert_eq!(track.album, "async");
    assert_eq!(track.file_type, "AAC audio file");
    assert_eq!(
        track.path,
        fake.mount.join("iPod_Control/Music/F07/ZXCV.m4a")
    );
}

#[test]
fn unset_optional_fields_are_none_or_empty() {
    let (_fake, tracks) = read_sample();

    let track = &tracks[1];

    assert_eq!(track.year, None);
    assert_eq!(track.track_number, None);
    assert_eq!(track.track_count, None);
    assert_eq!(track.disc_number, None);
    assert_eq!(track.disc_count, None);
    assert_eq!(track.last_played, None);
    assert_eq!(track.date_added, None);
    assert_eq!(track.album_artist, "");
    assert_eq!(track.composer, "");
    assert_eq!(track.sort_title, None);
    assert!(!track.compilation);
}

#[test]
fn podcast_urls_and_unknown_mhod_types_are_skipped() {
    let (_fake, tracks) = read_sample();

    let episode = &tracks[2];

    assert_eq!(episode.title, "Episode 42");
    assert_eq!(episode.album, "Some Show");
    assert_eq!(episode.media_kind, IpodMediaKind::Podcast);
}

#[test]
fn classifies_media_kinds_and_sums_usage_per_kind() {
    let fake = classic_7g(&sample_database());

    let database = read_database(&opened(&fake)).unwrap();

    let kinds: Vec<_> = database
        .tracks
        .iter()
        .map(|track| track.media_kind)
        .collect();
    assert_eq!(
        kinds,
        [
            IpodMediaKind::Music,
            IpodMediaKind::Music,
            IpodMediaKind::Podcast,
            IpodMediaKind::Audiobook,
            IpodMediaKind::Video
        ]
    );
    assert_eq!(database.usage.audio_bytes, 9_440_000 + 7_000_000);
    assert_eq!(database.usage.podcast_bytes, 30_000_000);
    assert_eq!(database.usage.audiobook_bytes, 50_000_000);
    assert_eq!(database.usage.video_bytes, 100_000_000);
}

#[test]
fn media_type_zero_is_music_unless_the_movie_flag_is_set() {
    let mut song = Track::new(1, "Song");
    song.media_type = 0;
    let mut movie = Track::new(2, "Movie");
    movie.media_type = 0;
    movie.movie_flag = true;

    let (_fake, tracks) = read_tracks(vec![song, movie]);

    assert_eq!(tracks[0].media_kind, IpodMediaKind::Music);
    assert_eq!(tracks[1].media_kind, IpodMediaKind::Video);
}

#[test]
fn converts_colon_separated_paths_to_native_paths_under_the_mount() {
    let (fake, tracks) = read_sample();

    assert_eq!(
        tracks[3].path,
        fake.mount
            .join("iPod_Control")
            .join("Music")
            .join("F03")
            .join("BOOK.m4b")
    );
    assert!(tracks.iter().all(|track| track.path.is_absolute()));
}

#[test]
fn path_components_cannot_escape_the_mount() {
    let mut hostile = Track::new(1, "Hostile");
    hostile.path = ":iPod_Control:..:..:etc:passwd".into();

    let (fake, tracks) = read_tracks(vec![hostile]);

    assert_eq!(
        tracks[0].path,
        fake.mount.join("iPod_Control").join("etc").join("passwd")
    );
}

#[test]
fn a_track_without_a_location_keeps_an_empty_path() {
    let mut nowhere = Track::new(1, "Nowhere");
    nowhere.path = String::new();

    let (_fake, tracks) = read_tracks(vec![nowhere]);

    assert_eq!(tracks[0].path, PathBuf::new());
}

#[test]
fn flash_ipods_store_the_file_type_as_a_four_character_code() {
    let mut track = Track::new(1, "Flash");
    track.file_type = None;
    track.file_type_marker = u32::from_be_bytes(*b"M4A ");

    let (_fake, tracks) = read_tracks(vec![track]);

    assert_eq!(tracks[0].file_type, "M4A");
}

#[test]
fn old_short_mhit_headers_leave_newer_fields_at_their_defaults() {
    let mut old = Track::new(1, "Old");
    old.artist = "Artist".into();
    old.skip_count = 9;
    old.media_type = MEDIA_VIDEO;
    let mut database = Database::new(vec![old], crate::sample_playlists());
    database.mhit_header = 0x9c;
    let fake = FakeIpod::new("OLD");
    fake.write_itunesdb(&database.build());

    let track = read_database(&opened(&fake)).unwrap().tracks.remove(0);

    assert_eq!(track.title, "Old");
    assert_eq!(track.artist, "Artist");
    assert_eq!(track.bitrate_kbps, Some(256));
    assert_eq!(track.skip_count, 0);
    assert_eq!(track.media_kind, IpodMediaKind::Music);
}

#[test]
fn a_string_that_overruns_its_mhod_is_corrupt() {
    let mut track = Track::new(1, "Overrun");
    let mut mhod = mhod_text(MHOD_PATH, "abcdef", Encoding::Utf8);
    // Claim a string far longer than the mhod holds.
    mhod[28..32].copy_from_slice(&1000u32.to_le_bytes());
    track.extra_mhods.push(mhod);
    let fake = FakeIpod::new("IPOD");
    fake.write_itunesdb(&Database::new(vec![track], crate::sample_playlists()).build());

    let error = read_database(&opened(&fake)).unwrap_err();

    assert!(
        matches!(error, antistreaming_ipod::Error::Corrupt { .. }),
        "{error}"
    );
}

#[test]
fn serializes_camel_case_with_64_bit_ids_as_strings() {
    let (_fake, tracks) = read_sample();

    let json = serde_json::to_value(&tracks[0]).unwrap();

    assert_eq!(json["dbid"], "18364758544493064720");
    assert_eq!(json["albumArtist"], "Beyoncé");
    assert_eq!(json["mediaKind"], "music");
    assert_eq!(json["durationMs"], 236_000);
    assert_eq!(json["bitrateKbps"], 320);
}
