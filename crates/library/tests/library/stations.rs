use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tempfile::TempDir;
use unsub_library::{
    Error, ExternalTrack, Library, MediaKind, NewStation, PlaylistKind, SmartField, SmartOp,
    SmartRule, SmartRules, SmartValue, SortKey, Station, Track, TrackEdits, TrackOverrides,
    TrackQuery,
};

use crate::common::{added_ids, all_kinds, fixtures_dir, library_of, rewind_schema, track};

const GROOVE_URL: &str = "https://ice1.somafm.com/groovesalad-128-mp3";
const GROOVE_UUID: &str = "960e57c5-0601-11e8-ae97-52543be04c81";

fn groove() -> NewStation {
    NewStation {
        name: "Groove Salad".into(),
        url: GROOVE_URL.into(),
        genre: "Ambient".into(),
        bitrate_kbps: Some(128),
        codec: "MP3".into(),
        homepage: Some("https://somafm.com/groovesalad/".into()),
        favicon_url: Some("https://somafm.com/img/groovesalad120.png".into()),
        radio_browser_uuid: Some(GROOVE_UUID.into()),
    }
}

fn named(name: &str, url: &str) -> NewStation {
    NewStation {
        name: name.into(),
        url: url.into(),
        ..NewStation::default()
    }
}

fn song(id: i64, title: &str) -> Track {
    track(id, title, "Artist", "Album", 200_000)
}

fn of_kind(kind: MediaKind, track: Track) -> Track {
    Track { kind, ..track }
}

fn ids(tracks: Vec<Track>) -> Vec<i64> {
    tracks.into_iter().map(|t| t.id).collect()
}

fn titles(tracks: Vec<Track>) -> Vec<String> {
    tracks.into_iter().map(|t| t.title).collect()
}

fn kind_query(kind: Option<MediaKind>) -> TrackQuery {
    TrackQuery {
        kind,
        sort: SortKey::Name,
        ..TrackQuery::default()
    }
}

fn listed(library: &Library, query: &TrackQuery) -> Vec<String> {
    titles(library.tracks(query).unwrap())
}

fn write(dir: &Path, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
}

fn invalid(result: Result<i64, Error>) -> bool {
    matches!(result, Err(Error::InvalidStation(_)))
}

// ---------------------------------------------------------------- storage

#[test]
fn a_station_is_a_radio_track_with_its_details() {
    let mut library = Library::open_in_memory().unwrap();

    let id = library.add_station(&groove()).unwrap();

    let row = library.track(id).unwrap().unwrap();
    assert_eq!(
        (
            row.kind,
            row.title.as_str(),
            row.path.as_str(),
            row.genre.as_str(),
            row.bitrate_kbps,
            row.codec.as_str()
        ),
        (
            MediaKind::Radio,
            "Groove Salad",
            GROOVE_URL,
            "Ambient",
            Some(128),
            "MP3"
        )
    );
    assert!(row.enabled && !row.in_cloud && !row.missing);
    assert!(!row.remember_position && !row.has_lyrics);
    assert_eq!((row.duration_ms, row.file_size, row.play_count), (0, 0, 0));
    assert_eq!(
        library.station(id).unwrap(),
        Some(Station {
            id,
            name: "Groove Salad".into(),
            url: GROOVE_URL.into(),
            genre: "Ambient".into(),
            bitrate_kbps: Some(128),
            codec: "MP3".into(),
            homepage: Some("https://somafm.com/groovesalad/".into()),
            favicon_url: Some("https://somafm.com/img/groovesalad120.png".into()),
            radio_browser_uuid: Some(GROOVE_UUID.into()),
            play_count: 0,
            last_played: None,
        })
    );
}

#[test]
fn stations_are_listed_by_name_and_only_stations_are_stations() {
    let mut library = library_of(&[song(1, "A Song")]);
    let zeta = library.add_station(&named("Zeta FM", "http://z/")).unwrap();
    let alpha = library
        .add_station(&named("alpha radio", "http://a/"))
        .unwrap();

    let names: Vec<String> = library
        .stations()
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();

    assert_eq!(names, ["alpha radio", "Zeta FM"]);
    assert!(
        library.station(1).unwrap().is_none(),
        "a song is no station"
    );
    assert!(library.station(999).unwrap().is_none());
    assert_eq!(
        ids(library.tracks(&kind_query(Some(MediaKind::Radio))).unwrap()),
        [alpha, zeta]
    );
}

#[test]
fn an_unnamed_station_is_called_by_its_address() {
    let mut library = Library::open_in_memory().unwrap();

    let id = library
        .add_station(&named("  \n ", "http://ice.example/live"))
        .unwrap();

    assert_eq!(
        library.track(id).unwrap().unwrap().title,
        "http://ice.example/live"
    );
}

#[test]
fn details_are_cleaned_and_unusable_ones_dropped() {
    let mut library = Library::open_in_memory().unwrap();

    let id = library
        .add_station(&NewStation {
            name: "  Soma\u{7} \t FM\n".into(),
            url: "  HTTP://Ice.Example/live  ".into(),
            genre: "x".repeat(1_000),
            bitrate_kbps: Some(0),
            codec: String::new(),
            homepage: Some("javascript:alert(1)".into()),
            favicon_url: Some("file:///etc/passwd".into()),
            radio_browser_uuid: Some("../../etc".into()),
        })
        .unwrap();

    let station = library.station(id).unwrap().unwrap();
    assert_eq!(station.name, "Soma FM");
    assert_eq!(station.url, "http://Ice.Example/live");
    assert_eq!(station.genre.chars().count(), 200);
    assert_eq!(
        (
            station.bitrate_kbps,
            station.homepage,
            station.favicon_url,
            station.radio_browser_uuid
        ),
        (None, None, None, None)
    );
}

#[test]
fn only_http_and_https_addresses_can_be_stations() {
    let mut library = Library::open_in_memory().unwrap();

    for url in [
        "",
        "   ",
        "ice.example.com/live",
        "ftp://host/a",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "mms://host/stream",
        "http://",
        "http://host/has space",
        "http://host/\u{0}",
        &format!("http://host/{}", "a".repeat(3_000)),
    ] {
        assert!(invalid(library.add_station(&named("Bad", url))), "{url:?}");
    }

    assert!(library.stations().unwrap().is_empty());
    assert!(library.tracks(&kind_query(None)).unwrap().is_empty());
}

// ------------------------------------------------------------------ dedupe

#[test]
fn the_same_address_is_the_same_station() {
    let mut library = Library::open_in_memory().unwrap();
    let first = library
        .add_station(&NewStation {
            genre: "Ambient".into(),
            ..named("Original", "http://ice.example/live")
        })
        .unwrap();

    let again = library
        .add_station(&NewStation {
            genre: "Other".into(),
            ..named("Different name", "  HTTP://ice.example/live ")
        })
        .unwrap();

    assert_eq!(again, first);
    let stations = library.stations().unwrap();
    assert_eq!(stations.len(), 1);
    assert_eq!(
        (stations[0].name.as_str(), stations[0].genre.as_str()),
        ("Original", "Ambient"),
        "the existing station is left as it was"
    );
    let other = library
        .add_station(&named("Other", "http://ice.example/other"))
        .unwrap();
    assert_ne!(other, first);
}

#[test]
fn the_same_directory_id_is_the_same_station_even_at_a_new_address() {
    let mut library = Library::open_in_memory().unwrap();
    let first = library.add_station(&groove()).unwrap();

    let moved = library
        .add_station(&NewStation {
            url: "https://ice2.somafm.com/groovesalad-256-mp3".into(),
            ..groove()
        })
        .unwrap();
    let rival = library
        .add_station(&NewStation {
            radio_browser_uuid: Some("00000000-0000-0000-0000-000000000001".into()),
            ..groove()
        })
        .unwrap();

    assert_eq!(moved, first);
    assert_eq!(
        rival, first,
        "the address matches, whatever its directory id"
    );
    let stations = library.stations().unwrap();
    assert_eq!(stations.len(), 1);
    assert_eq!(stations[0].url, GROOVE_URL);
}

#[test]
fn an_address_held_by_a_song_is_never_taken_over() {
    let mut library = library_of(&[Track {
        path: "http://ice.example/live".into(),
        ..song(1, "Not a station")
    }]);

    let result = library.add_station(&named("Live", "http://ice.example/live"));

    assert!(matches!(result, Err(Error::PathInUse(_))));
    let row = library.track(1).unwrap().unwrap();
    assert_eq!(
        (row.kind, row.title.as_str()),
        (MediaKind::Music, "Not a station")
    );
}

// ------------------------------------------------------- update and remove

#[test]
fn updating_a_station_keeps_its_place_and_history() {
    let mut library = Library::open_in_memory().unwrap();
    let id = library.add_station(&groove()).unwrap();
    let other = library.add_station(&named("Other", "http://o/")).unwrap();
    let list = library
        .create_playlist("Favourites", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library.add_to_playlist(list, &[other, id], None).unwrap();
    library.record_play(id, 1_000).unwrap();
    library.record_play(id, 2_000).unwrap();

    library
        .update_station(
            id,
            &NewStation {
                name: "Groove Salad Classic".into(),
                url: "https://ice6.somafm.com/gsclassic-128-mp3".into(),
                genre: "Downtempo".into(),
                bitrate_kbps: Some(256),
                codec: "AAC".into(),
                homepage: None,
                favicon_url: Some("https://somafm.com/new.png".into()),
                radio_browser_uuid: Some(GROOVE_UUID.into()),
            },
        )
        .unwrap();

    let station = library.station(id).unwrap().unwrap();
    assert_eq!(
        (
            station.name.as_str(),
            station.url.as_str(),
            station.genre.as_str(),
            station.bitrate_kbps,
            station.codec.as_str(),
            station.homepage.as_deref(),
            station.favicon_url.as_deref()
        ),
        (
            "Groove Salad Classic",
            "https://ice6.somafm.com/gsclassic-128-mp3",
            "Downtempo",
            Some(256),
            "AAC",
            None,
            Some("https://somafm.com/new.png")
        )
    );
    assert_eq!((station.play_count, station.last_played), (2, Some(2_000)));
    let playlist = TrackQuery {
        kind: None,
        playlist: Some(list),
        sort: SortKey::PlaylistOrder,
        ..TrackQuery::default()
    };
    assert_eq!(ids(library.tracks(&playlist).unwrap()), [other, id]);
    let found = TrackQuery {
        search: Some("classic".into()),
        ..kind_query(Some(MediaKind::Radio))
    };
    assert_eq!(listed(&library, &found), ["Groove Salad Classic"]);
    let old_genre = TrackQuery {
        search: Some("ambient".into()),
        ..kind_query(Some(MediaKind::Radio))
    };
    assert!(listed(&library, &old_genre).is_empty());
}

#[test]
fn updating_cannot_collide_with_another_station_or_touch_a_song() {
    let mut library = library_of(&[song(1, "A Song")]);
    let a = library.add_station(&groove()).unwrap();
    let b = library
        .add_station(&NewStation {
            radio_browser_uuid: Some("11111111-1111-1111-1111-111111111111".into()),
            ..named("B", "http://b/")
        })
        .unwrap();

    assert!(matches!(
        library.update_station(b, &named("B", GROOVE_URL)),
        Err(Error::PathInUse(_))
    ));
    assert!(matches!(
        library.update_station(
            b,
            &NewStation {
                radio_browser_uuid: Some(GROOVE_UUID.into()),
                ..named("B", "http://b/")
            }
        ),
        Err(Error::InvalidStation(_))
    ));
    assert!(matches!(
        library.update_station(b, &named("B", "ftp://b/")),
        Err(Error::InvalidStation(_))
    ));
    assert!(matches!(
        library.update_station(1, &named("Hijack", "http://x/")),
        Err(Error::TrackNotFound(1))
    ));
    assert!(matches!(
        library.update_station(999, &named("Ghost", "http://x/")),
        Err(Error::TrackNotFound(999))
    ));
    // Saving a station as it is, with its own address, is not a collision.
    library.update_station(a, &groove()).unwrap();
    assert_eq!(library.track(1).unwrap().unwrap().title, "A Song");
    assert_eq!(library.station(b).unwrap().unwrap().url, "http://b/");
}

#[test]
fn removing_a_station_takes_it_out_of_playlists_and_leaves_songs_alone() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let mut library = Library::open(&db_path).unwrap();
    library.import_tracks(&[song(1, "A Song")]).unwrap();
    let id = library.add_station(&groove()).unwrap();
    let list = library
        .create_playlist("Mixed", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library.add_to_playlist(list, &[1, id, id], None).unwrap();

    library.remove_station(id).unwrap();

    assert!(library.track(id).unwrap().is_none());
    assert!(library.station(id).unwrap().is_none());
    let playlist = TrackQuery {
        kind: None,
        playlist: Some(list),
        ..TrackQuery::default()
    };
    assert_eq!(ids(library.tracks(&playlist).unwrap()), [1]);
    let details: i64 = Connection::open(&db_path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM stations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(details, 0, "the station's details go with it");
    // Its directory id and address are free again.
    let again = library.add_station(&groove()).unwrap();
    assert!(library.station(again).unwrap().is_some());

    assert!(matches!(
        library.remove_station(1),
        Err(Error::TrackNotFound(1))
    ));
    assert!(matches!(
        library.remove_station(999),
        Err(Error::TrackNotFound(999))
    ));
    assert!(
        library.track(1).unwrap().is_some(),
        "a song is not removed as a station"
    );
}

// ------------------------------------------------------ playlists and plays

#[test]
fn playlists_hold_stations_beside_songs_in_order() {
    let mut library = library_of(&[song(1, "First Song"), song(2, "Second Song")]);
    let jazz = library
        .add_station(&named("Jazz FM", "http://jazz/"))
        .unwrap();
    let rock = library
        .add_station(&named("Rock FM", "http://rock/"))
        .unwrap();
    let list = library
        .create_playlist("Mixed", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library
        .add_to_playlist(list, &[2, jazz, 1, rock, jazz], None)
        .unwrap();
    let in_list = |kind| TrackQuery {
        kind,
        playlist: Some(list),
        sort: SortKey::PlaylistOrder,
        ..TrackQuery::default()
    };

    assert_eq!(
        ids(library.tracks(&in_list(None)).unwrap()),
        [2, jazz, 1, rock, jazz]
    );
    assert_eq!(
        ids(library.tracks(&in_list(Some(MediaKind::Music))).unwrap()),
        [2, 1],
        "asking for songs gets songs"
    );
    assert_eq!(
        ids(library.tracks(&in_list(Some(MediaKind::Radio))).unwrap()),
        [jazz, rock, jazz]
    );
    let stats = library.stats(&in_list(None)).unwrap();
    assert_eq!((stats.count, stats.total_duration_ms), (5, 400_000));

    library.remove_from_playlist(list, &[1, 4]).unwrap();
    assert_eq!(ids(library.tracks(&in_list(None)).unwrap()), [2, 1, rock]);
    library.move_in_playlist(list, &[2], 0).unwrap();
    assert_eq!(ids(library.tracks(&in_list(None)).unwrap()), [rock, 2, 1]);
}

#[test]
fn a_folder_lists_the_stations_of_the_playlists_in_it() {
    let mut library = library_of(&[song(1, "Tune")]);
    let station = library
        .add_station(&named("Jazz FM", "http://jazz/"))
        .unwrap();
    let folder = library
        .create_playlist("Folder", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    let inner = library
        .create_playlist("Inner", PlaylistKind::Regular, Some(folder))
        .unwrap()
        .id;
    library.add_to_playlist(inner, &[1, station], None).unwrap();

    let query = TrackQuery {
        kind: None,
        playlist: Some(folder),
        sort: SortKey::Name,
        ..TrackQuery::default()
    };

    assert_eq!(listed(&library, &query), ["Jazz FM", "Tune"]);
}

#[test]
fn plays_are_counted_and_the_last_stations_heard_are_listed() {
    let mut library = library_of(&[song(1, "A Song")]);
    let a = library.add_station(&named("A FM", "http://a/")).unwrap();
    let b = library.add_station(&named("B FM", "http://b/")).unwrap();
    let c = library.add_station(&named("C FM", "http://c/")).unwrap();
    let never = library.add_station(&named("Never", "http://n/")).unwrap();
    assert!(library.recently_played_stations(10).unwrap().is_empty());

    library.record_play(a, 1_000).unwrap();
    library.record_play(b, 2_000).unwrap();
    library.record_play(c, 3_000).unwrap();
    library.record_play(1, 4_000).unwrap();
    library.record_play(a, 5_000).unwrap();

    let recent = library.recently_played_stations(10).unwrap();
    assert_eq!(
        recent
            .iter()
            .map(|s| (s.id, s.play_count, s.last_played))
            .collect::<Vec<_>>(),
        [
            (a, 2, Some(5_000)),
            (c, 1, Some(3_000)),
            (b, 1, Some(2_000))
        ],
        "most recent first; songs and never-played stations are not listed"
    );
    assert!(recent.iter().all(|s| s.id != never));
    let two: Vec<i64> = library
        .recently_played_stations(2)
        .unwrap()
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(two, [a, c]);
    assert!(library.recently_played_stations(0).unwrap().is_empty());
    assert_eq!(library.track(a).unwrap().unwrap().play_count, 2);
    assert!(library.recent_plays(0).unwrap().contains(&(a, 5_000)));

    library.record_skip(b, 6_000).unwrap();
    assert_eq!(library.track(b).unwrap().unwrap().skip_count, 1);
    assert!(matches!(
        library.record_play(999, 1),
        Err(Error::TrackNotFound(999))
    ));
}

// --------------------------------------------------------------- isolation

/// Songs (one untagged, so it would share a group with anything blank),
/// a podcast, an audiobook, and two stations that look like they belong.
fn mixed() -> (Library, Vec<i64>) {
    let mut library = library_of(&[
        Track {
            genre: "Jazz".into(),
            ..song(1, "Morning Jazz")
        },
        track(2, "Untagged", "", "", 100_000),
        of_kind(MediaKind::Podcast, song(3, "Episode")),
        of_kind(MediaKind::Audiobook, song(4, "Chapter")),
    ]);
    let jazz = library
        .add_station(&NewStation {
            genre: "Jazz".into(),
            bitrate_kbps: Some(128),
            ..named("Jazz FM", "http://jazz/")
        })
        .unwrap();
    let blank = library
        .add_station(&named("Untagged", "http://blank/"))
        .unwrap();
    (library, vec![jazz, blank])
}

#[test]
fn song_podcast_and_audiobook_queries_never_return_stations() {
    let (library, stations) = mixed();
    let kind = |kind| listed(&library, &kind_query(kind));

    assert_eq!(kind(Some(MediaKind::Music)), ["Morning Jazz", "Untagged"]);
    assert_eq!(kind(Some(MediaKind::Podcast)), ["Episode"]);
    assert_eq!(kind(Some(MediaKind::Audiobook)), ["Chapter"]);
    assert_eq!(
        kind(None),
        ["Chapter", "Episode", "Morning Jazz", "Untagged"],
        "every kind of file, not stations"
    );
    assert_eq!(kind(Some(MediaKind::Radio)), ["Jazz FM", "Untagged"]);
    assert_eq!(library.tracks(&TrackQuery::default()).unwrap().len(), 2);

    let search = |kind, text: &str| {
        listed(
            &library,
            &TrackQuery {
                search: Some(text.into()),
                ..kind_query(kind)
            },
        )
    };
    assert_eq!(search(None, "jazz"), ["Morning Jazz"]);
    assert_eq!(search(Some(MediaKind::Radio), "jazz"), ["Jazz FM"]);
    assert_eq!(search(None, "untagged"), ["Untagged"]);
    let by_ids = TrackQuery {
        ids: Some(stations.clone()),
        ..kind_query(None)
    };
    assert_eq!(
        library.tracks(&by_ids).unwrap(),
        [],
        "asking by id does not widen it"
    );

    assert_eq!(library.stats(&kind_query(None)).unwrap().count, 4);
    assert_eq!(
        library
            .stats(&kind_query(Some(MediaKind::Radio)))
            .unwrap()
            .count,
        2
    );
}

#[test]
fn albums_artists_genres_and_composers_leave_stations_out() {
    let (mut library, stations) = mixed();
    let list = library
        .create_playlist("Mixed", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library
        .add_to_playlist(list, &[1, 2, stations[0], stations[1]], None)
        .unwrap();
    let everything = kind_query(None);
    let in_playlist = TrackQuery {
        playlist: Some(list),
        ..kind_query(None)
    };

    for query in [&everything, &in_playlist] {
        let albums = library.albums(query).unwrap();
        assert_eq!(
            albums.iter().map(|a| a.track_count).sum::<u32>(),
            if query.playlist.is_some() { 2 } else { 4 },
            "{query:?}"
        );
        let artists = library.artists(query).unwrap();
        assert_eq!(
            artists.iter().map(|a| a.track_count).sum::<u32>(),
            if query.playlist.is_some() { 2 } else { 4 },
            "{artists:?}"
        );
        let genres = library.genres(query).unwrap();
        assert_eq!(
            genres
                .iter()
                .map(|g| (g.name.as_str(), g.track_count))
                .collect::<Vec<_>>(),
            [("Jazz", 1)],
            "the station's Jazz is not counted"
        );
        assert!(
            library
                .composers(query)
                .unwrap()
                .iter()
                .all(|c| c.track_count <= 4)
        );
    }

    // The untagged song has no artist and no album, as the stations do not;
    // looking its album up must not bring them along.
    let untagged = library
        .albums(&everything)
        .unwrap()
        .into_iter()
        .find(|album| album.album.is_empty())
        .unwrap();
    assert_eq!(untagged.track_count, 1);
    assert_eq!(ids(library.album_tracks(&untagged.key).unwrap()), [2]);
}

#[test]
fn smart_playlists_ignore_stations_unless_a_rule_names_radio() {
    use SmartField::{Genre, Name, Playlist};
    use SmartOp::*;
    let mut library = library_of(&[
        Track {
            genre: "Jazz".into(),
            ..song(1, "Zulu Jazz")
        },
        Track {
            genre: "Rock".into(),
            ..song(2, "Yankee Rock")
        },
        of_kind(MediaKind::Podcast, song(3, "Episode")),
    ]);
    let jazz = library
        .add_station(&NewStation {
            genre: "Jazz".into(),
            ..named("Alpha FM", "http://alpha/")
        })
        .unwrap();
    library
        .add_station(&NewStation {
            genre: "Rock".into(),
            ..named("Bravo FM", "http://bravo/")
        })
        .unwrap();
    let regular = library
        .create_playlist("Regular", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library.add_to_playlist(regular, &[1, jazz], None).unwrap();
    let smart = library
        .create_playlist("Smart", PlaylistKind::Smart, None)
        .unwrap()
        .id;
    let condition = |field, op, value: SmartValue| SmartRule::Condition { field, op, value };
    let text = |value: &str| SmartValue::Text(value.into());
    let evaluate = |library: &mut Library, rules: SmartRules| {
        library.set_smart_rules(smart, &rules).unwrap();
        listed(
            library,
            &TrackQuery {
                kind: None,
                playlist: Some(smart),
                sort: SortKey::Name,
                ..TrackQuery::default()
            },
        )
    };
    let all = |rules: Vec<SmartRule>| SmartRules {
        rules,
        ..SmartRules::default()
    };
    let any = |rules: Vec<SmartRule>| SmartRules {
        match_all: false,
        rules,
        ..SmartRules::default()
    };

    assert_eq!(
        evaluate(&mut library, all(vec![])),
        ["Episode", "Yankee Rock", "Zulu Jazz"],
        "no rules is every song, not every station"
    );
    assert_eq!(
        evaluate(&mut library, all(vec![condition(Genre, Is, text("jazz"))])),
        ["Zulu Jazz"]
    );
    assert_eq!(
        evaluate(
            &mut library,
            all(vec![condition(Name, Contains, text("FM"))])
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        evaluate(
            &mut library,
            all(vec![condition(SmartField::MediaKind, IsNot, text("music"))])
        ),
        ["Episode"],
        "not-music does not mean stations"
    );
    assert_eq!(
        evaluate(
            &mut library,
            all(vec![condition(Playlist, Is, SmartValue::Playlist(regular))])
        ),
        ["Zulu Jazz"]
    );
    let limited = SmartRules {
        limit: Some(unsub_library::SmartLimit {
            amount: 1,
            unit: unsub_library::LimitUnit::Items,
            selected_by: unsub_library::LimitOrder::Name,
        }),
        ..all(vec![])
    };
    assert_eq!(
        evaluate(&mut library, limited),
        ["Episode"],
        "a limit counts songs, so a station cannot take the place"
    );

    // Naming radio brings them in, however the rule is written or nested.
    assert_eq!(
        evaluate(
            &mut library,
            all(vec![condition(SmartField::MediaKind, Is, text("radio"))])
        ),
        ["Alpha FM", "Bravo FM"]
    );
    assert_eq!(
        evaluate(
            &mut library,
            all(vec![condition(SmartField::MediaKind, Is, text(" RADIO "))])
        ),
        ["Alpha FM", "Bravo FM"]
    );
    assert_eq!(
        evaluate(
            &mut library,
            all(vec![
                condition(SmartField::MediaKind, Is, text("radio")),
                condition(Genre, Is, text("rock")),
            ])
        ),
        ["Bravo FM"]
    );
    assert_eq!(
        evaluate(
            &mut library,
            any(vec![
                condition(SmartField::MediaKind, Is, text("radio")),
                condition(Genre, Is, text("rock")),
            ])
        ),
        ["Alpha FM", "Bravo FM", "Yankee Rock"]
    );
    assert_eq!(
        evaluate(
            &mut library,
            all(vec![SmartRule::Group(any(vec![
                condition(SmartField::MediaKind, Is, text("radio")),
                condition(SmartField::MediaKind, Is, text("podcast")),
            ]))])
        ),
        ["Alpha FM", "Bravo FM", "Episode"]
    );
    assert_eq!(
        evaluate(
            &mut library,
            all(vec![condition(SmartField::MediaKind, IsNot, text("radio"))])
        ),
        ["Episode", "Yankee Rock", "Zulu Jazz"]
    );
    assert_eq!(
        evaluate(
            &mut library,
            all(vec![condition(SmartField::MediaKind, Is, text("music"))])
        ),
        ["Yankee Rock", "Zulu Jazz"]
    );

    for bad in [
        condition(SmartField::MediaKind, Is, text("video")),
        condition(SmartField::MediaKind, Contains, text("radio")),
        condition(SmartField::MediaKind, Is, SmartValue::Number(3)),
    ] {
        assert!(matches!(
            library.set_smart_rules(smart, &all(vec![bad])),
            Err(Error::InvalidRule(_))
        ));
    }
}

#[test]
fn nothing_that_works_on_files_touches_stations() {
    let dir = TempDir::new().unwrap();
    let real = dir.path().join("music/Artist/Album/01 Real.mp3");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::copy(fixtures_dir().join("halo.mp3"), &real).unwrap();
    let mut library = Library::open_in_memory().unwrap();
    let trashed = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
    let recorder = Arc::clone(&trashed);
    library.set_trash_handler(move |path| {
        recorder.lock().unwrap().push(path.to_path_buf());
        Ok(())
    });
    let song_id = added_ids(&mut library, &[(real.clone(), TrackOverrides::default())])[0];
    let a = library
        .add_station(&NewStation {
            genre: "Jazz".into(),
            ..named("Halo", "http://a/")
        })
        .unwrap();
    let b = library.add_station(&named("Halo", "http://b/")).unwrap();
    let songs = TrackQuery {
        kind: None,
        ..TrackQuery::default()
    };

    // Missing-file checks.
    assert!(library.check_missing().unwrap().is_empty());
    fs::remove_file(&real).unwrap();
    assert_eq!(library.check_missing().unwrap(), [song_id]);
    fs::copy(fixtures_dir().join("halo.mp3"), &real).unwrap();
    assert!(library.check_missing().unwrap().is_empty());
    assert!(!library.track(a).unwrap().unwrap().missing);

    // Duplicates and matching, even when edited to look like songs.
    library
        .edit_tracks(
            &[a, b],
            &TrackEdits {
                artist: Some("Halo Artist".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    assert_eq!(library.duplicates(false).unwrap(), Vec::<Vec<i64>>::new());
    assert_eq!(library.duplicates(true).unwrap(), Vec::<Vec<i64>>::new());
    let external = |title: &str, artist: &str| ExternalTrack {
        external_id: "x".into(),
        title: title.into(),
        artist: artist.into(),
        album_artist: String::new(),
        album: String::new(),
        duration_ms: None,
        track_number: None,
        disc_number: None,
        path: None,
        heard_ms: None,
    };
    let matched = library
        .match_external(&[external("Halo", "Halo Artist"), external("Halo", "")])
        .unwrap();
    assert!(
        matched.iter().flatten().all(|m| m.track_id == song_id),
        "{matched:?}"
    );

    // Loudness, cloud status, artwork, chapters, lyrics.
    let needing: Vec<i64> = library
        .tracks_needing_loudness(100)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(needing, [song_id]);
    assert!(
        library
            .sound_check_gain(a, unsub_library::GainMode::Track, -16.0)
            .unwrap()
            .is_none()
    );
    assert!(
        library
            .sound_check_gain(a, unsub_library::GainMode::Album, -16.0)
            .unwrap()
            .is_none()
    );
    assert!(library.refresh_cloud_status(&[a, b]).unwrap().is_empty());
    assert!(library.artwork(a).unwrap().is_none());
    assert!(library.artwork_thumbnail(a, 100).unwrap().is_none());
    assert!(library.chapters(a).unwrap().is_empty());
    assert!(library.lyrics(a).unwrap().is_none());

    // Organising looks only at files, and a media folder does not claim stations.
    library.set_media_folder(&dir.path().join("music")).unwrap();
    let mut seen = 0;
    let report = library
        .organize_now(|progress| seen = progress.total)
        .unwrap();
    assert_eq!(seen, 1);
    assert!(report.failed.is_empty());
    assert_eq!(library.track(a).unwrap().unwrap().path, "http://a/");

    // Deleting with files to the Trash trashes the song's file and no station's.
    library.delete_tracks(&[a, song_id], true).unwrap();
    assert_eq!(trashed.lock().unwrap().len(), 1);
    assert!(library.track(a).unwrap().is_none());
    assert!(library.track(b).unwrap().is_some());
    assert!(library.tracks(&songs).unwrap().is_empty());
}

#[test]
fn editing_a_station_through_track_edits_changes_the_library_only() {
    let (mut library, stations) = mixed();
    let jazz = stations[0];

    let report = library
        .edit_tracks(
            &[jazz],
            &TrackEdits {
                genre: Some("Smooth Jazz".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert!(report.written.is_empty());
    assert!(
        report.db_only[0].1.contains("no file"),
        "{:?}",
        report.db_only
    );
    let row = library.track(jazz).unwrap().unwrap();
    assert_eq!(
        (row.genre.as_str(), row.kind),
        ("Smooth Jazz", MediaKind::Radio)
    );

    for (ids, kind) in [
        (vec![jazz], MediaKind::Music),
        (vec![1], MediaKind::Radio),
        (vec![1, jazz], MediaKind::Podcast),
    ] {
        let result = library.edit_tracks(
            &ids,
            &TrackEdits {
                kind: Some(kind),
                ..TrackEdits::default()
            },
        );
        assert!(
            matches!(result, Err(Error::InvalidStation(_))),
            "{ids:?} {kind:?}"
        );
    }
    assert_eq!(library.track(1).unwrap().unwrap().kind, MediaKind::Music);
    assert_eq!(library.track(jazz).unwrap().unwrap().kind, MediaKind::Radio);
}

#[test]
fn a_file_cannot_be_added_as_a_station_by_override_or_scan() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("halo.mp3");
    fs::copy(fixtures_dir().join("halo.mp3"), &path).unwrap();
    let mut library = Library::open_in_memory().unwrap();
    let station = library.add_station(&groove()).unwrap();

    let id = added_ids(
        &mut library,
        &[(
            path,
            TrackOverrides {
                kind: Some(MediaKind::Radio),
                ..TrackOverrides::default()
            },
        )],
    )[0];
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(library.track(id).unwrap().unwrap().kind, MediaKind::Music);
    assert!(summary.failed.is_empty());
    let kept = library.station(station).unwrap().unwrap();
    assert_eq!(kept.url, GROOVE_URL, "a scan leaves stations as they are");
    assert_eq!(library.tracks(&all_kinds()).unwrap().len(), 1);
}

#[test]
fn mixes_never_offer_stations() {
    let mut songs = Vec::new();
    for artist in 0..10 {
        for n in 0..4 {
            songs.push(Track {
                album_artist: String::new(),
                genre: "Rock".into(),
                ..track(
                    100 + artist * 4 + n,
                    &format!("song {artist}-{n}"),
                    &format!("Artist {artist}"),
                    &format!("Album {artist}"),
                    200_000,
                )
            });
        }
    }
    let mut library = library_of(&songs);
    let station = library
        .add_station(&NewStation {
            genre: "Rock".into(),
            ..named("Rock FM", "http://rock/")
        })
        .unwrap();
    for n in 0..30 {
        library
            .record_play(station, 1_700_000_000 + n * 60)
            .unwrap();
        library
            .record_play(100 + n % 10, 1_700_000_030 + n * 60)
            .unwrap();
    }

    let mixes = library.mixes(5, 20).unwrap();

    assert!(!mixes.is_empty());
    assert!(mixes.iter().all(|mix| !mix.track_ids.contains(&station)));
    assert!(!library.mix_for(&[100, 101], 20).unwrap().contains(&station));
}

// ------------------------------------------------------------------ import

#[test]
fn a_pls_file_becomes_stations() {
    let dir = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();
    let path = write(
        dir.path(),
        "stations.pls",
        "\u{feff}[playlist]\r\nNumberOfEntries=3\r\n\
         File2=http://ice.example/two\r\nTitle2=Two FM\r\nLength2=-1\r\n\
         File1=https://ice.example/one\r\nTitle1=One FM\r\n\
         File3=http://ice.example/three\r\n\
         Version=2\r\n",
    );

    let ids = library.import_playlist_file(&path).unwrap();

    assert_eq!(ids.len(), 3);
    let names: Vec<(String, String)> = ids
        .iter()
        .map(|id| {
            let station = library.station(*id).unwrap().unwrap();
            (station.name, station.url)
        })
        .collect();
    assert_eq!(
        names,
        [
            ("One FM".to_owned(), "https://ice.example/one".to_owned()),
            ("Two FM".to_owned(), "http://ice.example/two".to_owned()),
            (
                "http://ice.example/three".to_owned(),
                "http://ice.example/three".to_owned()
            ),
        ]
    );
    assert!(
        ids.iter()
            .all(|id| library.track(*id).unwrap().unwrap().kind == MediaKind::Radio)
    );
}

#[test]
fn an_m3u_file_becomes_stations_and_importing_again_adds_nothing() {
    let dir = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();
    let existing = library
        .add_station(&named("Mine", "http://ice.example/two"))
        .unwrap();
    let path = write(
        dir.path(),
        "stations.m3u8",
        "#EXTM3U\n#EXTINF:-1,One FM\nhttp://ice.example/one\n\
         #EXTINF:-1 tvg-id=\"x\",Two FM\nhttp://ice.example/two\n\
         http://ice.example/one\n\n#EXTINF:-1,Three\nhttps://ice.example/three\n",
    );

    let first = library.import_playlist_file(&path).unwrap();
    let second = library.import_playlist_file(&path).unwrap();

    assert_eq!(first.len(), 3, "a repeated line is one station");
    assert_eq!(second, first);
    assert_eq!(
        first[1], existing,
        "a known address is the station already here"
    );
    assert_eq!(library.stations().unwrap().len(), 3);
    assert_eq!(library.station(existing).unwrap().unwrap().name, "Mine");
}

#[test]
fn only_web_addresses_are_imported_from_a_playlist_file() {
    let dir = TempDir::new().unwrap();
    let mut library = library_of(&[song(1, "A Song")]);
    let path = write(
        dir.path(),
        "mixed.m3u",
        "#EXTM3U\nfile:///etc/passwd\njavascript:alert(1)\nftp://host/a\nmms://host/b\n\
         data:audio/mp3;base64,AAAA\n/Users/me/Music/song.mp3\nC:\\Music\\song.mp3\n\
         ../up/song.mp3\nsong.mp3\nhttp://\nhttp://host with space/x\nhttps:///nohost\n\
         http://good.example/live\n",
    );

    let ids = library.import_playlist_file(&path).unwrap();

    assert_eq!(ids.len(), 1);
    assert_eq!(
        library.station(ids[0]).unwrap().unwrap().url,
        "http://good.example/live"
    );
    assert_eq!(library.tracks(&kind_query(None)).unwrap().len(), 1);
}

#[test]
fn a_playlist_of_songs_or_a_stream_manifest_is_not_a_station_list() {
    let dir = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();
    let songs = write(
        dir.path(),
        "songs.m3u",
        "#EXTM3U\n#EXTINF:123,Artist - Song\n/Users/me/a.mp3\nrelative/b.flac\n",
    );
    let hls = write(
        dir.path(),
        "live.m3u8",
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nhttp://cdn.example/seg1.ts\n\
         #EXTINF:10,\nhttp://cdn.example/seg2.ts\n",
    );
    let empty = write(dir.path(), "empty.pls", "");
    let noise = write(
        dir.path(),
        "noise.pls",
        (0..=255u8).cycle().take(5_000).collect::<Vec<_>>(),
    );

    for file in [songs, hls, empty, noise] {
        assert!(
            library.import_playlist_file(&file).unwrap().is_empty(),
            "{file:?}"
        );
    }
    assert!(library.stations().unwrap().is_empty());
    assert!(matches!(
        library.import_playlist_file(&dir.path().join("missing.pls")),
        Err(Error::Io(_))
    ));
}

#[test]
fn hostile_station_lists_are_bounded() {
    let dir = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();

    // Thousands of entries: a thousand are taken, in the file's order.
    let mut many = String::from("[playlist]\n");
    for n in 1..=5_000 {
        many.push_str(&format!(
            "File{n}=http://h.example/{n}\nTitle{n}=Station {n}\n"
        ));
    }
    let ids = library
        .import_playlist_file(&write(dir.path(), "many.pls", &many))
        .unwrap();
    assert_eq!(ids.len(), 1_000);
    assert_eq!(library.station(ids[0]).unwrap().unwrap().name, "Station 1");
    assert_eq!(
        library.station(ids[999]).unwrap().unwrap().name,
        "Station 1000"
    );
    assert_eq!(library.stations().unwrap().len(), 1_000);

    // Only the first megabyte of a file is read.
    let mut big = String::from("#EXTM3U\nhttp://early.example/live\n");
    big.push_str(&"#padding to push the rest past the limit\n".repeat(30_000));
    big.push_str("http://late.example/live\n");
    assert!(big.len() > 1024 * 1024);
    let ids = library
        .import_playlist_file(&write(dir.path(), "big.m3u", &big))
        .unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(
        library.station(ids[0]).unwrap().unwrap().url,
        "http://early.example/live"
    );

    // One enormous line or name, and control characters in names.
    let long_url = format!("http://h.example/{}", "a".repeat(100_000));
    let hostile = format!(
        "#EXTM3U\n#EXTINF:-1,{}\nhttp://named.example/x\n{long_url}\n#EXTINF:-1,Tab\tand\0nul\u{7}\nhttp://ctl.example/x\n",
        "N".repeat(50_000)
    );
    let ids = library
        .import_playlist_file(&write(dir.path(), "hostile.m3u", &hostile))
        .unwrap();
    assert_eq!(ids.len(), 2, "the oversized address is skipped");
    assert_eq!(
        library
            .station(ids[0])
            .unwrap()
            .unwrap()
            .name
            .chars()
            .count(),
        200
    );
    assert_eq!(library.station(ids[1]).unwrap().unwrap().name, "Tab andnul");
}

// --------------------------------------------------------------- migration

#[test]
fn a_library_from_before_stations_upgrades_and_keeps_everything() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    {
        let mut library = Library::open(&db_path).unwrap();
        library
            .import_tracks(&[
                song(1, "A Song"),
                of_kind(MediaKind::Podcast, song(2, "Pod")),
            ])
            .unwrap();
        let list = library
            .create_playlist("Mine", PlaylistKind::Regular, None)
            .unwrap()
            .id;
        library.add_to_playlist(list, &[1, 2], None).unwrap();
        library.record_play(1, 5_000).unwrap();
    }
    rewind_schema(&db_path, 9);
    let before: i64 = Connection::open(&db_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'stations'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(before, 0);

    let mut library = Library::open(&db_path).unwrap();

    let version: u32 = Connection::open(&db_path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 11);
    assert_eq!(
        listed(&library, &kind_query(Some(MediaKind::Music))),
        ["A Song"]
    );
    assert_eq!(
        listed(&library, &kind_query(Some(MediaKind::Podcast))),
        ["Pod"]
    );
    assert_eq!(library.track(1).unwrap().unwrap().play_count, 1);
    assert!(
        library
            .playlists()
            .unwrap()
            .iter()
            .any(|p| p.name == "Mine")
    );
    let id = library.add_station(&groove()).unwrap();
    assert_eq!(library.station(id).unwrap().unwrap().name, "Groove Salad");
    assert_eq!(library.stations().unwrap().len(), 1);
}

// -------------------------------------------------------------------- wire

#[test]
fn stations_cross_the_wire_in_camel_case() {
    let new: NewStation = serde_json::from_str(
        r#"{"name":"N","url":"http://u/","bitrateKbps":96,"radioBrowserUuid":"abc-1","faviconUrl":null}"#,
    )
    .unwrap();
    assert_eq!(
        (
            new.bitrate_kbps,
            new.radio_browser_uuid.as_deref(),
            new.favicon_url
        ),
        (Some(96), Some("abc-1"), None)
    );
    assert_eq!(
        serde_json::from_str::<NewStation>("{}").unwrap(),
        NewStation::default()
    );

    let mut library = Library::open_in_memory().unwrap();
    let id = library.add_station(&groove()).unwrap();
    let json = serde_json::to_value(library.station(id).unwrap().unwrap()).unwrap();
    assert_eq!(json["bitrateKbps"], 128);
    assert_eq!(json["radioBrowserUuid"], GROOVE_UUID);
    assert_eq!(
        json["faviconUrl"],
        "https://somafm.com/img/groovesalad120.png"
    );
    assert_eq!(json["lastPlayed"], serde_json::Value::Null);

    assert_eq!(serde_json::to_value(MediaKind::Radio).unwrap(), "radio");
    let query: TrackQuery = serde_json::from_str(r#"{"kind":"radio"}"#).unwrap();
    assert_eq!(query.kind, Some(MediaKind::Radio));
    let rule: SmartRule = serde_json::from_str(
        r#"{"type":"condition","field":"mediaKind","op":"is","value":{"text":"radio"}}"#,
    )
    .unwrap();
    assert!(matches!(
        rule,
        SmartRule::Condition {
            field: SmartField::MediaKind,
            ..
        }
    ));
}
