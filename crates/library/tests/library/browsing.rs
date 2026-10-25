use crate::common::{library_of, track};
use unsub_library::{
    Library, MediaKind, PlaylistKind, SmartField, SmartOp, SmartRule, SmartRules, SmartValue,
    SortKey, Track, TrackQuery,
};

fn catalogue() -> Library {
    library_of(&[
        Track {
            album_artist: String::new(),
            genre: "Röck".into(),
            composer: "Dvořák".into(),
            year: Some(2001),
            track_number: Some(2),
            disc_number: Some(1),
            ..track(1, "First Song", "Ann", "Shared", 1000)
        },
        Track {
            album_artist: "Ánn".into(),
            genre: "rock".into(),
            composer: "Dvorak".into(),
            year: Some(2002),
            track_number: Some(1),
            disc_number: Some(1),
            ..track(2, "Second Song", "Guest", "Shared", 2000)
        },
        Track {
            genre: "Rock".into(),
            ..track(3, "Third Song", "Annabelle", "Other", 3000)
        },
        Track {
            album_artist: "Ann".into(),
            genre: "Pop".into(),
            year: Some(2010),
            ..track(4, "Fourth Song", "Guest", "Later", 4000)
        },
        track(5, "Unknown Song", "Ann", "Undated", 5000),
        Track {
            compilation: true,
            genre: "Jazz".into(),
            ..track(6, "Sixth Song", "Eve", "Hits", 6000)
        },
        Track {
            compilation: true,
            genre: "jazz".into(),
            ..track(7, "Seventh Song", "Fay", "Hits", 7000)
        },
        Track {
            kind: MediaKind::Podcast,
            ..track(8, "Eighth Song", "Ann", "Show", 8000)
        },
    ])
}
fn ids(library: &Library, query: &TrackQuery) -> Vec<i64> {
    library
        .tracks(query)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect()
}
#[test]
fn exact_keys_use_existing_fallback_normalization_and_compilation_groups() {
    let library = catalogue();
    let query = TrackQuery {
        artist_key: Some("ann".into()),
        sort: SortKey::Name,
        ..TrackQuery::default()
    };
    assert_eq!(ids(&library, &query), [1, 4, 2, 5]);
    assert!(
        !ids(&library, &query).contains(&3),
        "Annabelle must not match Ann"
    );
    assert_eq!(library.artists(&query).unwrap().len(), 1);
    let compilation = TrackQuery {
        artist_key: Some("compilations".into()),
        ..TrackQuery::default()
    };
    assert_eq!(ids(&library, &compilation), [6, 7]);
    assert_eq!(
        library.artists(&compilation).unwrap()[0].name,
        "Compilations"
    );
    assert_eq!(library.albums(&compilation).unwrap()[0].track_count, 2);
    let hostile = TrackQuery {
        artist_key: Some("ann' OR 1=1 --".into()),
        ..TrackQuery::default()
    };
    assert!(ids(&library, &hostile).is_empty());
}
#[test]
fn browser_filters_intersect_search_kind_ids_and_duplicate_playlist_entries() {
    let mut library = catalogue();
    let playlist = library
        .create_playlist("Scoped", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library
        .add_to_playlist(playlist, &[1, 1, 2, 3, 6, 8], None)
        .unwrap();
    let query = TrackQuery {
        artist_key: Some("ann".into()),
        album_key: Some("ann\u{1f}shared".into()),
        genre: Some(" ROCK ".into()),
        composer: Some("DVOŘÁK".into()),
        search: Some("song".into()),
        playlist: Some(playlist),
        ids: Some(vec![1, 2, 8]),
        sort: SortKey::PlaylistOrder,
        ..TrackQuery::default()
    };
    assert_eq!(ids(&library, &query), [1, 1, 2]);
    let stats = library.stats(&query).unwrap();
    assert_eq!((stats.count, stats.total_duration_ms), (3, 4000));
    let artists = library.artists(&query).unwrap();
    assert_eq!(
        (
            artists.len(),
            artists[0].track_count,
            artists[0].album_count
        ),
        (1, 2, 1)
    );
    assert_eq!(artists[0].artwork_track_ids, [1]);
    assert_eq!(library.albums(&query).unwrap()[0].track_count, 2);
    assert_eq!(
        library
            .album_tracks_scoped("ann\u{1f}shared", &query)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        [2, 1, 1]
    );
    assert!(
        library
            .album_tracks_scoped("ann\u{1f}later", &query)
            .unwrap()
            .is_empty()
    );
    assert!(
        library
            .artist_albums_scoped("annabelle", &query)
            .unwrap()
            .is_empty()
    );
    assert!(
        ids(
            &library,
            &TrackQuery {
                search: Some("missing".into()),
                ..query.clone()
            }
        )
        .is_empty()
    );
    assert!(
        ids(
            &library,
            &TrackQuery {
                kind: Some(MediaKind::Podcast),
                ..query
            }
        )
        .is_empty()
    );
}
#[test]
fn browser_filters_apply_to_smart_playlists_and_folder_membership() {
    let mut library = catalogue();
    let folder = library
        .create_playlist("Folder", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    let regular = library
        .create_playlist("Regular", PlaylistKind::Regular, Some(folder))
        .unwrap()
        .id;
    library
        .add_to_playlist(regular, &[1, 1, 2, 3, 8], None)
        .unwrap();
    let smart = library
        .create_playlist("Smart", PlaylistKind::Smart, Some(folder))
        .unwrap()
        .id;
    library
        .set_smart_rules(
            smart,
            &SmartRules {
                rules: vec![SmartRule::Condition {
                    field: SmartField::Name,
                    op: SmartOp::Is,
                    value: SmartValue::Text("First Song".into()),
                }],
                ..SmartRules::default()
            },
        )
        .unwrap();
    let query = TrackQuery {
        artist_key: Some("ann".into()),
        genre: Some("rock".into()),
        sort: SortKey::Name,
        playlist: Some(smart),
        ..TrackQuery::default()
    };
    assert_eq!(ids(&library, &query), [1]);
    assert_eq!(
        ids(
            &library,
            &TrackQuery {
                playlist: Some(folder),
                ..query.clone()
            }
        ),
        [1, 2]
    );
    assert_eq!(
        library
            .stats(&TrackQuery {
                playlist: Some(folder),
                ..query
            })
            .unwrap()
            .count,
        2
    );
}
#[test]
fn artist_albums_are_chronological_scoped_and_unknown_years_are_last() {
    let library = catalogue();
    let albums = library
        .artist_albums_scoped("ann", &TrackQuery::default())
        .unwrap();
    assert_eq!(
        albums.iter().map(|a| a.album.as_str()).collect::<Vec<_>>(),
        ["Shared", "Later", "Undated"]
    );
    assert_eq!(albums[0].year, Some(2001));
    let query = TrackQuery {
        ids: Some(vec![2, 4, 8]),
        search: Some("song".into()),
        ..TrackQuery::default()
    };
    let scoped = library.artist_albums_scoped("ann", &query).unwrap();
    assert_eq!(
        scoped.iter().map(|a| a.track_count).collect::<Vec<_>>(),
        [1, 1]
    );
    assert_eq!(scoped[0].year, Some(2002));
    let artists = library.artists(&query).unwrap();
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0].artwork_track_ids, [2, 4]);
    assert_eq!(artists[0].track_count, 2);
}
#[test]
fn artwork_mosaics_are_bounded_and_use_distinct_scoped_albums() {
    let tracks: Vec<Track> = (1..=20)
        .map(|id| Track {
            year: Some(2000 + id as i32),
            ..track(id, "Song", "Artist", &format!("Album {}", id / 2), 1000)
        })
        .collect();
    let library = library_of(&tracks);
    let artists = library.artists(&TrackQuery::default()).unwrap();
    assert_eq!(artists[0].artwork_track_ids, [1, 2, 4, 6]);
    let scoped = TrackQuery {
        ids: Some(vec![10, 11, 12]),
        ..TrackQuery::default()
    };
    assert_eq!(
        library.artists(&scoped).unwrap()[0].artwork_track_ids,
        [10, 12]
    );
}
#[test]
fn new_sorts_are_numeric_or_folded_with_missing_values_last_and_stable_ties() {
    let library = library_of(&[
        Track {
            composer: "Zulu".into(),
            track_number: Some(2),
            disc_number: Some(2),
            skip_count: 9,
            ..track(1, "One", "Artist", "Album", 1000)
        },
        Track {
            composer: "Ábel".into(),
            track_number: Some(1),
            disc_number: Some(1),
            skip_count: 1,
            ..track(2, "Two", "Artist", "Album", 1000)
        },
        Track {
            composer: "zulu".into(),
            track_number: Some(2),
            disc_number: Some(1),
            skip_count: 4,
            ..track(3, "Three", "Artist", "Album", 1000)
        },
        track(4, "Four", "Artist", "Album", 1000),
    ]);
    for (sort, ascending, descending) in [
        (SortKey::Composer, vec![2, 3, 1, 4], vec![3, 1, 2, 4]),
        (SortKey::TrackNumber, vec![2, 3, 1, 4], vec![3, 1, 2, 4]),
        (SortKey::DiscNumber, vec![2, 3, 1, 4], vec![1, 2, 3, 4]),
        (SortKey::SkipCount, vec![4, 2, 3, 1], vec![1, 3, 2, 4]),
    ] {
        assert_eq!(
            ids(
                &library,
                &TrackQuery {
                    sort,
                    ..TrackQuery::default()
                }
            ),
            ascending,
            "{sort:?} ascending"
        );
        assert_eq!(
            ids(
                &library,
                &TrackQuery {
                    sort,
                    descending: true,
                    ..TrackQuery::default()
                }
            ),
            descending,
            "{sort:?} descending"
        );
    }
}
#[test]
fn browser_query_json_defaults_preserve_existing_clients() {
    let old: TrackQuery = serde_json::from_str(r#"{"sort":"artist"}"#).unwrap();
    assert_eq!(old, TrackQuery::default());
    let scoped: TrackQuery = serde_json::from_str(r#"{"artistKey":"ann","albumKey":"ann\u001fshared","genre":"Rock","composer":"Bach","sort":"skipCount"}"#).unwrap();
    assert_eq!(scoped.artist_key.as_deref(), Some("ann"));
    assert_eq!(scoped.album_key.as_deref(), Some("ann\u{1f}shared"));
    assert_eq!(scoped.sort, SortKey::SkipCount);
    let serialized = serde_json::to_value(scoped).unwrap();
    assert_eq!(serialized["artistKey"], "ann");
    assert_eq!(serialized["composer"], "Bach");
}

#[test]
fn genre_keys_stay_stable_when_scope_changes_the_display_spelling() {
    let library = catalogue();
    let first = library
        .genres(&TrackQuery {
            ids: Some(vec![1]),
            ..TrackQuery::default()
        })
        .unwrap();
    let second = library
        .genres(&TrackQuery {
            ids: Some(vec![2]),
            ..TrackQuery::default()
        })
        .unwrap();
    assert_ne!(first[0].name, second[0].name);
    assert_eq!(first[0].key, "rock");
    assert_eq!(first[0].key, second[0].key);
    assert_eq!(
        library
            .stats(&TrackQuery {
                genre: Some(first[0].key.clone()),
                ..TrackQuery::default()
            })
            .unwrap()
            .count,
        3
    );
}

#[test]
fn large_catalogue_facets_keep_artwork_bounded_and_exact_scope_consistent() {
    // Exercise real SQLite grouping at fixture scale without reading any music files.
    let tracks: Vec<Track> = (1..=12_000)
        .map(|id| {
            let artist = (id - 1) / 60;
            let album = ((id - 1) % 60) / 10;
            Track {
                genre: if id % 2 == 0 { "Rock" } else { "Jazz" }.into(),
                year: Some(1990 + album as i32),
                track_number: Some(((id - 1) % 10 + 1) as u32),
                ..track(
                    id,
                    "Song",
                    &format!("Artist {artist}"),
                    &format!("Album {album}"),
                    1000,
                )
            }
        })
        .collect();
    let library = library_of(&tracks);
    let artists = library.artists(&TrackQuery::default()).unwrap();
    assert_eq!(artists.len(), 200);
    assert!(
        artists
            .iter()
            .all(|a| a.track_count == 60 && a.album_count == 6 && a.artwork_track_ids.len() == 4)
    );
    let query = TrackQuery {
        artist_key: Some("artist 37".into()),
        genre: Some("rock".into()),
        ..TrackQuery::default()
    };
    assert_eq!(ids(&library, &query).len(), 30);
    assert_eq!(library.stats(&query).unwrap().count, 30);
    let albums = library.artist_albums_scoped("artist 37", &query).unwrap();
    assert_eq!(albums.len(), 6);
    assert!(albums.iter().all(|a| a.track_count == 5));
    assert_eq!(
        library.artists(&query).unwrap()[0].artwork_track_ids.len(),
        4
    );
}
