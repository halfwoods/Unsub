use tempfile::TempDir;
use unsub_library::{Library, MediaKind, PlaylistKind, SortKey, Track, TrackQuery};

use crate::common::{
    all_kinds, copy_fixtures, library_of, rewind_schema, scan, scanned_library, track,
};

const SEP: char = '\u{1f}';

/// Fourteen music tracks across a dozen artists and albums, plus one podcast,
/// built to hit each grouping rule.
fn catalogue() -> Library {
    let row = |id, title: &str, artist: &str, album: &str| track(id, title, artist, album, 10_000);
    let tracks = [
        // Ann's "First": two discs, tracks out of id order, mixed years and genres.
        Track {
            track_number: Some(2),
            disc_number: Some(1),
            year: Some(1990),
            genre: "Rock".into(),
            composer: "Bach".into(),
            date_added: 100,
            album_artist: String::new(),
            ..row(1, "t1", "Ann", "First")
        },
        Track {
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(1991),
            genre: "Rock".into(),
            composer: "bach".into(),
            date_added: 300,
            duration_ms: 20_000,
            album_artist: String::new(),
            ..row(2, "t2", "Ann", "First")
        },
        Track {
            track_number: Some(1),
            disc_number: Some(2),
            date_added: 200,
            duration_ms: 30_000,
            album_artist: String::new(),
            ..row(3, "t3", "Ann", "First")
        },
        // Same album name, different artist: a different album.
        Track {
            year: Some(2000),
            genre: "pop".into(),
            composer: "Bach".into(),
            album_artist: String::new(),
            ..row(4, "u1", "Bob", "First")
        },
        // The album artist, not the track artist, decides the group.
        Track {
            year: Some(1985),
            genre: "Pop".into(),
            composer: "Dvo\u{159}\u{e1}k".into(),
            album_artist: "Cat & Co".into(),
            ..row(5, "v1", "Cat", "Second")
        },
        Track {
            composer: "Dvorak".into(),
            album_artist: "Cat & Co".into(),
            ..row(6, "v2", "Dan", "Second")
        },
        // Flagged compilations pool under "Compilations"; an unflagged "Hits" does not join them.
        Track {
            compilation: true,
            genre: "Jazz".into(),
            album_artist: String::new(),
            ..row(7, "w1", "Eve", "Hits")
        },
        Track {
            compilation: true,
            genre: "Jazz".into(),
            album_artist: String::new(),
            ..row(8, "w2", "Fay", "Hits")
        },
        Track {
            album_artist: String::new(),
            ..row(9, "x1", "Gus", "Hits")
        },
        Track {
            year: Some(1979),
            genre: "rock".into(),
            album_artist: String::new(),
            ..row(10, "wall", "Pink Floyd", "The Wall")
        },
        Track {
            album_artist: String::new(),
            ..row(11, "n1", "Hal", "")
        },
        Track {
            kind: MediaKind::Podcast,
            album_artist: String::new(),
            ..row(12, "s1", "Ivy", "Show")
        },
        Track {
            album_artist: String::new(),
            ..row(13, "z1", "The Zombies", "Odessey")
        },
        Track {
            album_artist: String::new(),
            ..row(14, "o1", "", "Orphans")
        },
        Track {
            year: Some(1999),
            album_artist: String::new(),
            ..row(15, "l1", "Ann", "Later")
        },
        Track {
            year: Some(1980),
            album_artist: String::new(),
            ..row(16, "e1", "Ann", "Early")
        },
    ];
    library_of(&tracks)
}

fn sorted(sort: SortKey, descending: bool) -> TrackQuery {
    TrackQuery {
        sort,
        descending,
        ..TrackQuery::default()
    }
}

fn album_names(library: &Library, query: &TrackQuery) -> Vec<String> {
    library
        .albums(query)
        .unwrap()
        .into_iter()
        .map(|a| a.album)
        .collect()
}

#[test]
fn albums_group_by_album_artist_falling_back_to_artist() {
    let library = catalogue();
    let albums = library.albums(&TrackQuery::default()).unwrap();

    let by_artist: Vec<(&str, &str, u32)> = albums
        .iter()
        .map(|a| (a.album_artist.as_str(), a.album.as_str(), a.track_count))
        .collect();
    assert_eq!(
        by_artist,
        [
            ("Ann", "Early", 1),
            ("Ann", "First", 3),
            ("Ann", "Later", 1),
            ("Bob", "First", 1),
            ("Cat & Co", "Second", 2),
            ("Compilations", "Hits", 2),
            ("Gus", "Hits", 1),
            ("Hal", "", 1),
            ("Pink Floyd", "The Wall", 1),
            ("The Zombies", "Odessey", 1),
            ("", "Orphans", 1),
        ]
    );
}

#[test]
fn album_summaries_aggregate_their_tracks() {
    let library = catalogue();
    let albums = library.albums(&TrackQuery::default()).unwrap();
    let first = albums
        .iter()
        .find(|a| a.album_artist == "Ann" && a.album == "First")
        .unwrap();
    let hits = albums
        .iter()
        .find(|a| a.album_artist == "Compilations")
        .unwrap();
    let gus = albums.iter().find(|a| a.album_artist == "Gus").unwrap();

    assert_eq!(first.key, format!("ann{SEP}first"));
    assert_eq!(first.duration_ms, 60_000);
    assert_eq!(first.year, Some(1990), "the earliest year on the album");
    assert_eq!(first.genre, "Rock", "the first non-empty genre");
    assert_eq!(first.artwork_track_id, 1, "the first-added track");
    assert_eq!(first.date_added, 300, "the latest addition");
    assert!(!first.compilation);
    assert_eq!(hits.key, format!("compilations{SEP}hits"));
    assert!(hits.compilation);
    assert_eq!((hits.track_count, hits.genre.as_str()), (2, "Jazz"));
    assert!(
        !gus.compilation && gus.key != hits.key,
        "an unflagged Hits stays apart"
    );
    assert_eq!(gus.year, None);
}

#[test]
fn tracks_carry_the_key_of_the_album_they_are_on() {
    let library = catalogue();
    let albums = library.albums(&TrackQuery::default()).unwrap();
    let tracks = library.tracks(&TrackQuery::default()).unwrap();

    for album in &albums {
        let on_it = tracks.iter().filter(|t| t.album_group == album.key).count();
        assert_eq!(on_it, album.track_count as usize, "{}", album.key);
    }
    let group_of = |id| &tracks.iter().find(|t| t.id == id).unwrap().album_group;
    assert_eq!(group_of(1), &format!("ann{SEP}first"));
    assert_eq!(group_of(7), &format!("compilations{SEP}hits"));

    // Spelling variants of one album share a group, which tags compared as written would not.
    let variants = library_of(&[
        track(1, "a", "Beyonc\u{e9}", "Lemonade", 1_000),
        track(2, "b", "BEYONCE", "lemonade", 1_000),
    ]);
    let tracks = variants.tracks(&TrackQuery::default()).unwrap();
    let albums = variants.albums(&TrackQuery::default()).unwrap();
    assert_eq!(albums.len(), 1);
    assert!(tracks.iter().all(|t| t.album_group == albums[0].key));
}

#[test]
fn albums_sort_by_artist_or_by_album_and_blanks_come_last() {
    let library = catalogue();

    assert_eq!(
        album_names(&library, &sorted(SortKey::Artist, false)),
        [
            "Early", "First", "Later", "First", "Second", "Hits", "Hits", "", "The Wall",
            "Odessey", "Orphans"
        ]
    );
    assert_eq!(
        album_names(&library, &sorted(SortKey::AlbumArtist, false)),
        album_names(&library, &sorted(SortKey::Artist, false))
    );
    // "The Wall" files under W; the album with no name goes last.
    assert_eq!(
        album_names(&library, &sorted(SortKey::Album, false)),
        [
            "Early", "First", "First", "Hits", "Hits", "Later", "Odessey", "Orphans", "Second",
            "The Wall", ""
        ]
    );
    assert_eq!(
        album_names(&library, &sorted(SortKey::Album, true)),
        [
            "The Wall", "Second", "Orphans", "Odessey", "Later", "Hits", "Hits", "First", "First",
            "Early", ""
        ],
        "descending flips the album key; ties keep ascending artist order"
    );
    assert_eq!(
        album_names(&library, &sorted(SortKey::Name, false)),
        album_names(&library, &sorted(SortKey::Album, false)),
        "other keys use album order"
    );
    let artists_descending: Vec<String> = library
        .albums(&sorted(SortKey::Artist, true))
        .unwrap()
        .into_iter()
        .map(|a| a.album_artist)
        .collect();
    assert_eq!(
        artists_descending.first().map(String::as_str),
        Some("The Zombies")
    );
    assert_eq!(artists_descending.last().map(String::as_str), Some(""));
}

#[test]
fn albums_honour_the_query_filters() {
    let mut library = catalogue();

    let podcasts = TrackQuery {
        kind: Some(MediaKind::Podcast),
        ..TrackQuery::default()
    };
    assert_eq!(album_names(&library, &podcasts), ["Show"]);
    assert_eq!(album_names(&library, &all_kinds()).len(), 12);

    let searching = TrackQuery {
        search: Some("ann".into()),
        ..TrackQuery::default()
    };
    let found = library.albums(&searching).unwrap();
    assert_eq!(
        found.iter().map(|a| a.album.as_str()).collect::<Vec<_>>(),
        ["Early", "First", "Later"]
    );

    // Only the matching tracks count towards an album.
    let by_ids = TrackQuery {
        ids: Some(vec![1, 5]),
        ..TrackQuery::default()
    };
    let counts: Vec<(String, u32)> = library
        .albums(&by_ids)
        .unwrap()
        .into_iter()
        .map(|a| (a.album, a.track_count))
        .collect();
    assert_eq!(counts, [("First".to_owned(), 1), ("Second".to_owned(), 1)]);

    // A playlist entry repeated still counts a track once.
    let playlist = library
        .create_playlist("P", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library
        .add_to_playlist(playlist, &[1, 1, 2, 1], None)
        .unwrap();
    let in_playlist = TrackQuery {
        playlist: Some(playlist),
        ..TrackQuery::default()
    };
    let albums = library.albums(&in_playlist).unwrap();
    assert_eq!((albums.len(), albums[0].track_count), (1, 2));
    assert_eq!(library.artists(&in_playlist).unwrap()[0].track_count, 2);
}

#[test]
fn album_tracks_come_in_disc_and_track_order_whatever_their_kind() {
    let library = catalogue();
    let titles = |key: String| -> Vec<String> {
        library
            .album_tracks(&key)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    };

    assert_eq!(titles(format!("ann{SEP}first")), ["t2", "t1", "t3"]);
    assert_eq!(titles(format!("compilations{SEP}hits")), ["w1", "w2"]);
    assert_eq!(titles(format!("gus{SEP}hits")), ["x1"]);
    assert_eq!(titles(format!("ivy{SEP}show")), ["s1"], "podcasts included");
    assert_eq!(titles(format!("{SEP}orphans")), ["o1"]);
    assert!(titles("nobody".to_owned()).is_empty());
}

#[test]
fn artists_count_their_albums_and_tracks() {
    let library = catalogue();

    let artists = library.artists(&TrackQuery::default()).unwrap();

    let summary: Vec<(&str, u32, u32)> = artists
        .iter()
        .map(|a| (a.name.as_str(), a.album_count, a.track_count))
        .collect();
    assert_eq!(
        summary,
        [
            ("Ann", 3, 5),
            ("Bob", 1, 1),
            ("Cat & Co", 1, 2),
            ("Compilations", 1, 2),
            ("Gus", 1, 1),
            ("Hal", 1, 1),
            ("Pink Floyd", 1, 1),
            ("The Zombies", 1, 1),
            ("", 1, 1),
        ]
    );
    assert_eq!(artists[0].key, "ann");
    assert_eq!(artists[3].key, "compilations");
    let reversed: Vec<String> = library
        .artists(&sorted(SortKey::Artist, true))
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(
        reversed,
        [
            "The Zombies",
            "Pink Floyd",
            "Hal",
            "Gus",
            "Compilations",
            "Cat & Co",
            "Bob",
            "Ann",
            ""
        ]
    );
    let searched = TrackQuery {
        search: Some("Cat".into()),
        ..TrackQuery::default()
    };
    assert_eq!(library.artists(&searched).unwrap().len(), 1);
}

#[test]
fn an_artists_albums_run_oldest_first() {
    let library = catalogue();

    let albums = library.artist_albums("ann").unwrap();

    assert_eq!(
        albums
            .iter()
            .map(|a| (a.album.as_str(), a.year))
            .collect::<Vec<_>>(),
        [
            ("Early", Some(1980)),
            ("First", Some(1990)),
            ("Later", Some(1999))
        ]
    );
    let compilations = library.artist_albums("compilations").unwrap();
    assert_eq!(compilations.len(), 1);
    assert!(compilations[0].compilation);
    assert!(library.artist_albums("nobody").unwrap().is_empty());
}

#[test]
fn genres_merge_spellings_and_leave_out_blanks() {
    let library = catalogue();

    let genres = library.genres(&TrackQuery::default()).unwrap();

    let summary: Vec<(&str, u32, u32)> = genres
        .iter()
        .map(|g| (g.name.as_str(), g.album_count, g.track_count))
        .collect();
    assert_eq!(summary, [("Jazz", 1, 2), ("Pop", 2, 2), ("Rock", 2, 3)]);
    let reversed: Vec<String> = library
        .genres(&sorted(SortKey::Genre, true))
        .unwrap()
        .into_iter()
        .map(|g| g.name)
        .collect();
    assert_eq!(reversed, ["Rock", "Pop", "Jazz"]);
    let searched = TrackQuery {
        search: Some("ann".into()),
        ..TrackQuery::default()
    };
    let in_search = library.genres(&searched).unwrap();
    assert_eq!(in_search.len(), 1);
    assert_eq!(
        (in_search[0].name.as_str(), in_search[0].track_count),
        ("Rock", 2)
    );
}

#[test]
fn composers_merge_spellings_and_leave_out_blanks() {
    let library = catalogue();

    let composers = library.composers(&TrackQuery::default()).unwrap();

    let summary: Vec<(&str, u32)> = composers
        .iter()
        .map(|c| (c.name.as_str(), c.track_count))
        .collect();
    assert_eq!(summary, [("Bach", 3), ("Dvorak", 2)]);
    let reversed = library.composers(&sorted(SortKey::Name, true)).unwrap();
    assert_eq!(reversed[0].name, "Dvorak");
    let narrowed = TrackQuery {
        ids: Some(vec![2, 5]),
        ..TrackQuery::default()
    };
    assert_eq!(library.composers(&narrowed).unwrap().len(), 2);
}

#[test]
fn scanned_files_group_by_their_tags_and_compilation_flag() {
    let (library, _dir) = scanned_library();

    let albums = library.albums(&TrackQuery::default()).unwrap();

    let summary: Vec<(&str, &str, u32, bool)> = albums
        .iter()
        .map(|a| {
            (
                a.album_artist.as_str(),
                a.album.as_str(),
                a.track_count,
                a.compilation,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("The Beatles", "Abbey Road", 2, false),
            ("Compilations", "I Am... Sasha Fierce", 1, true),
            ("The Cranberries", "No Need to Argue", 1, false),
            ("Queen", "A Night at the Opera", 1, false),
            ("", "", 1, false),
        ]
    );
    let abbey_road: Vec<String> = library
        .album_tracks(&albums[0].key)
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(abbey_road, ["Come Together", "Something"]);
    assert!(
        albums
            .iter()
            .all(|a| library.track(a.artwork_track_id).unwrap().is_some())
    );
}

#[test]
fn rescans_and_the_migration_backfill_agree_on_group_keys() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let fixtures = copy_fixtures();
    let (albums, artists) = {
        let mut library = Library::open(&db_path).unwrap();
        scan(&mut library, fixtures.path());
        (
            library.albums(&all_kinds()).unwrap(),
            library.artists(&all_kinds()).unwrap(),
        )
    };
    assert_eq!(albums.len(), 7);

    // Rows written before the group columns existed get them from SQL.
    rewind_schema(&db_path, 2);
    let mut library = Library::open(&db_path).unwrap();

    assert_eq!(library.albums(&all_kinds()).unwrap(), albums);
    assert_eq!(library.artists(&all_kinds()).unwrap(), artists);
    assert!(
        library
            .tracks(&all_kinds())
            .unwrap()
            .iter()
            .all(|t| t.enabled)
    );
    let rescan = library
        .add_paths(&[fixtures.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!(rescan.unchanged, 8);
    assert_eq!(library.albums(&all_kinds()).unwrap(), albums);
}
