use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use unsub_elsewhere::{Error, Play, Playlist, Service, Song, read};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn song(
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration_ms: Option<u64>,
    id: Option<&str>,
) -> Song {
    Song {
        title: title.into(),
        artist: artist.into(),
        album: album.map(Into::into),
        duration_ms,
        service_id: id.map(Into::into),
    }
}

fn play(at: i64, title: &str, artist: &str, album: Option<&str>, ms_played: Option<u64>) -> Play {
    Play {
        at,
        title: title.into(),
        artist: artist.into(),
        album: album.map(Into::into),
        ms_played,
    }
}

fn titles(playlist: &Playlist) -> Vec<&str> {
    playlist
        .songs
        .iter()
        .map(|song| song.title.as_str())
        .collect()
}

#[test]
fn spotify_account_data_gives_playlists_in_order_and_a_years_plays() {
    let export = read(Service::Spotify, &fixture("spotify-account")).unwrap();

    let names: Vec<&str> = export.playlists.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Road Trip", "Empty", "Liked Songs"]);
    // The episode is left out. A local file is read from its URI, if it names an artist and title.
    assert_eq!(
        export.playlists[0].songs,
        [
            song(
                "Fixture Song",
                "The Fixtures",
                Some("Test Album"),
                None,
                Some("spotify:track:aaa")
            ),
            song(
                "Second Song",
                "Sample Band",
                Some("Second Album"),
                None,
                Some("spotify:track:bbb")
            ),
            Song {
                duration_ms: Some(100_000),
                ..song(
                    "Demo Tape",
                    "Home",
                    Some("Demo"),
                    None,
                    Some("spotify:local:Home:Demo:Demo+Tape:100")
                )
            },
            Song {
                duration_ms: Some(215_000),
                ..song(
                    "Café",
                    "Björk, Someone",
                    Some("Home Demos"),
                    None,
                    Some("spotify:local:Bj%C3%B6rk%2C+Someone:Home+Demos:Caf%C3%A9:215")
                )
            },
        ]
    );
    assert_eq!(
        export.playlists[2].songs,
        [song(
            "Fixture Song",
            "The Fixtures",
            Some("Test Album"),
            None,
            Some("spotify:track:aaa")
        )]
    );
    // The 12-second play is under Spotify's 30 seconds, and the podcast file isn't music.
    assert_eq!(
        export.plays,
        [
            play(
                1_709_325_840,
                "Fixture Song",
                "The Fixtures",
                None,
                Some(200_000)
            ),
            play(
                1_709_367_000,
                "Second Song",
                "Sample Band",
                None,
                Some(180_000)
            ),
        ]
    );
    assert_eq!((export.short_plays, export.unnamed), (1, 0));
}

#[test]
fn extended_streaming_history_gives_albums_and_skips_podcasts_and_short_plays() {
    // The first play ended at 20:44:12, which is filed under its minute like the account data's.
    let export = read(Service::Spotify, &fixture("spotify-extended")).unwrap();

    assert!(export.playlists.is_empty());
    assert_eq!(
        export.plays,
        [
            play(
                1_709_325_840,
                "Fixture Song",
                "The Fixtures",
                Some("Test Album"),
                Some(200_000)
            ),
            play(
                1_709_367_000,
                "Second Song",
                "Sample Band",
                Some("Second Album"),
                Some(30_000)
            ),
            play(
                1_709_546_400,
                "Loose Single",
                "Sample Band",
                None,
                Some(240_000)
            ),
        ]
    );
    assert_eq!(export.short_plays, 1);
}

#[test]
fn the_extended_history_replaces_the_account_datas_year_instead_of_adding_to_it() {
    let both = TempDir::new().unwrap();
    for (from, name) in [
        ("spotify-account", "StreamingHistory_music_0.json"),
        ("spotify-account", "Playlist1.json"),
        (
            "spotify-extended",
            "Streaming_History_Audio_2023-2024_0.json",
        ),
    ] {
        fs::copy(fixture(from).join(name), both.path().join(name)).unwrap();
    }

    let export = read(Service::Spotify, both.path()).unwrap();

    assert_eq!(export.plays.len(), 3);
    assert_eq!(
        export
            .plays
            .iter()
            .filter(|play| play.title == "Fixture Song")
            .count(),
        1
    );
    assert_eq!(export.playlists.len(), 2);
}

#[test]
fn a_play_in_both_spotify_exports_is_the_same_play() {
    let account = read(Service::Spotify, &fixture("spotify-account")).unwrap();
    let extended = read(Service::Spotify, &fixture("spotify-extended")).unwrap();

    let first = |plays: &[Play]| {
        plays
            .iter()
            .find(|play| play.title == "Fixture Song")
            .map(|play| play.at)
    };
    assert_eq!(first(&account.plays), first(&extended.plays));
}

#[test]
fn exportify_csvs_are_playlists_named_after_their_files() {
    let export = read(Service::Spotify, &fixture("exportify")).unwrap();

    let names: Vec<&str> = export.playlists.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["liked songs", "road trip"]);
    let liked = &export.playlists[0];
    assert_eq!(
        liked.songs[0],
        song(
            "Second Song",
            "Sample Band",
            Some("Second Album"),
            Some(215_000),
            Some("bbb")
        )
    );
    assert_eq!(liked.songs[1].artist, "Sample Band, Guest Singer");
    assert_eq!(liked.songs[1].album, None);
    let road = &export.playlists[1];
    assert_eq!(
        road.songs[0],
        song(
            "Fixture Song",
            "The Fixtures",
            Some("Test Album"),
            Some(200_500),
            Some("spotify:track:aaa")
        )
    );
    assert_eq!(road.songs[1].artist, "The Fixtures, Tyler, The Fixture");
    assert!(export.plays.is_empty());
}

#[test]
fn exportify_files_in_other_languages_are_read_by_their_translated_labels_or_their_order() {
    let export = read(Service::Spotify, &fixture("exportify-translated")).unwrap();

    let names: Vec<&str> = export.playlists.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["canciones", "unknown language"]);
    assert_eq!(
        export.playlists[0].songs,
        [song(
            "Fixture Song",
            "The Fixtures",
            Some("Test Album"),
            Some(200_500),
            Some("spotify:track:aaa")
        )]
    );
    // Labels nobody has listed: the columns are where Exportify always puts them, as a first
    // row that is a local file doesn't change.
    assert_eq!(titles(&export.playlists[1]), ["Demo Tape", "Second Song"]);
    assert_eq!(
        export.playlists[1].songs[1],
        song(
            "Second Song",
            "Sample Band",
            Some("Second Album"),
            Some(215_000),
            Some("spotify:track:bbb")
        )
    );
}

#[test]
fn older_extended_histories_are_called_endsong() {
    let export = read(Service::Spotify, &fixture("spotify-endsong")).unwrap();

    assert_eq!(
        export.plays,
        [play(
            1_462_357_200,
            "Old Song",
            "Sample Band",
            Some("Old Album"),
            Some(250_000)
        )]
    );
}

#[test]
fn one_exportify_file_can_be_chosen_on_its_own() {
    let export = read(Service::Spotify, &fixture("exportify/road_trip.csv")).unwrap();

    assert_eq!(export.playlists.len(), 1);
    assert_eq!(
        titles(&export.playlists[0]),
        ["Fixture Song", "Duet, Part 1"]
    );
}

#[test]
fn takeout_names_playlist_videos_from_the_history_and_library_and_keeps_the_music_plays() {
    let export = read(Service::YoutubeMusic, &fixture("takeout")).unwrap();

    // Not the cooking video, the removed video, the ad, the music video from a record label's
    // channel, or the entry in a language whose "Watched" is not known.
    assert_eq!(
        export.plays,
        [
            play(1_709_325_852, "Fixture Song", "The Fixtures", None, None),
            play(
                1_709_366_400 + 600,
                "Second Song",
                "Sample Band",
                None,
                None
            ),
        ]
    );
    let names: Vec<&str> = export.playlists.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["music library songs", "Road Trip"]);
    let library = &export.playlists[0];
    assert_eq!(
        library.songs,
        [song(
            "Library Song",
            "Sample Band, Guest Singer",
            Some("Library Album"),
            None,
            Some("vid0004")
        )]
    );
    // Listed by id only: two the history knows, one the library knows, and one nothing knows.
    let road = &export.playlists[1];
    assert_eq!(
        titles(road),
        ["Second Song", "Fixture Song", "Library Song"]
    );
    assert_eq!(export.unnamed, 1);
}

#[test]
fn listenbrainz_reads_the_export_zips_monthly_files_and_ignores_the_rest() {
    let export = read(Service::ListenBrainz, &fixture("listenbrainz")).unwrap();

    assert_eq!(
        export.plays,
        [
            play(
                1_709_325_852,
                "Fixture Song",
                "The Fixtures",
                Some("Test Album"),
                None
            ),
            play(1_709_366_400, "No Album Song", "Sample Band", None, None),
        ]
    );
    assert!(export.playlists.is_empty());
}

#[test]
fn listenbrainz_reads_the_api_form_of_a_listen_list() {
    let export = read(
        Service::ListenBrainz,
        &fixture("listenbrainz-api/listens.json"),
    )
    .unwrap();

    assert_eq!(
        export.plays,
        [play(
            1_709_325_852,
            "Fixture Song",
            "The Fixtures",
            Some("Test Album"),
            None
        )]
    );
}

#[test]
fn a_folder_for_another_service_has_nothing_to_read() {
    for (service, folder) in [
        (Service::YoutubeMusic, "spotify-account"),
        (Service::ListenBrainz, "takeout"),
        (Service::Spotify, "listenbrainz"),
    ] {
        let error = read(service, &fixture(folder)).unwrap_err();
        assert!(
            matches!(error, Error::NothingFound(found) if found == service),
            "{service:?} in {folder}"
        );
    }
}

#[test]
fn a_file_that_is_not_json_names_the_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Playlist1.json"), "{ not json").unwrap();

    let error = read(Service::Spotify, dir.path()).unwrap_err();

    assert!(error.to_string().contains("Playlist1.json"), "{error}");
}

#[test]
fn a_missing_path_is_an_error() {
    assert!(matches!(
        read(Service::Spotify, Path::new("/no/such/export")),
        Err(Error::Io(_))
    ));
}
