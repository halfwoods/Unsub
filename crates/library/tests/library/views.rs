use crate::common::{library_of, track};
use unsub_library::{Library, MediaKind, PlaylistKind, SortKey, Track, TrackQuery};

fn row(id: i64, title: &str, artist: &str, album: &str) -> Track {
    Track {
        album_artist: String::new(),
        ..track(id, title, artist, album, 10_000)
    }
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
fn new_metadata_sorts_are_numeric_normalized_and_keep_missing_values_last() {
    let library = library_of(&[
        Track {
            composer: "Zèd".into(),
            track_number: Some(2),
            disc_number: Some(1),
            skip_count: 3,
            ..row(1, "A", "Ann", "First")
        },
        Track {
            composer: "Anna".into(),
            track_number: Some(1),
            disc_number: Some(2),
            skip_count: 9,
            ..row(2, "B", "Ann", "First")
        },
        Track {
            track_number: None,
            disc_number: None,
            skip_count: 0,
            ..row(3, "C", "Ann", "First")
        },
    ]);
    for (sort, ascending, descending) in [
        (SortKey::Composer, vec![2, 1, 3], vec![1, 2, 3]),
        (SortKey::TrackNumber, vec![2, 1, 3], vec![1, 2, 3]),
        (SortKey::DiscNumber, vec![1, 2, 3], vec![2, 1, 3]),
        (SortKey::Skips, vec![3, 1, 2], vec![2, 1, 3]),
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
            "{sort:?}"
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
            "{sort:?}"
        );
    }
}

#[test]
fn exact_artist_filters_use_album_artist_fallback_normalization_and_compilations() {
    let library = library_of(&[
        row(1, "First", "Beyoncé", "One"),
        Track {
            album_artist: "Beyonce".into(),
            ..row(2, "Second", "Guest", "Two")
        },
        row(3, "Beyonce tribute", "Other", "Beyonce"),
        Track {
            compilation: true,
            ..row(4, "Hit A", "Beyonce", "Hits")
        },
        Track {
            compilation: true,
            ..row(5, "Hit B", "Another", "Hits")
        },
    ]);
    let query = TrackQuery {
        artist_group: Some("beyonce".into()),
        ..TrackQuery::default()
    };
    assert_eq!(ids(&library, &query), [1, 2]);
    let artists = library.artists(&query).unwrap();
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0].track_count, 2);
    assert_eq!(artists[0].artwork_track_ids.len(), 2);
    let compilations = TrackQuery {
        artist_group: Some("compilations".into()),
        ..TrackQuery::default()
    };
    assert_eq!(ids(&library, &compilations), [5, 4]);
    assert_eq!(library.albums(&compilations).unwrap().len(), 1);
    assert!(library.albums(&compilations).unwrap()[0].compilation);
    assert!(
        ids(
            &library,
            &TrackQuery {
                artist_group: Some("beyonce' OR 1=1 --".into()),
                ..query
            }
        )
        .is_empty()
    );
}

#[test]
fn browser_filters_combine_with_search_playlist_kind_and_duplicate_entries() {
    let mut library = library_of(&[
        Track {
            genre: "Ròck".into(),
            composer: "Bach".into(),
            ..row(1, "Needle", "Ann", "One")
        },
        Track {
            genre: "Pop".into(),
            ..row(2, "Needle Two", "Ann", "One")
        },
        Track {
            genre: "Rock".into(),
            ..row(3, "Needle Three", "Ann", "Two")
        },
        Track {
            genre: "Rock".into(),
            ..row(4, "Needle Four", "Anna", "One")
        },
        Track {
            genre: "Rock".into(),
            kind: MediaKind::Podcast,
            ..row(5, "Needle Show", "Ann", "One")
        },
        Track {
            genre: "Rock".into(),
            ..row(6, "Other song", "Ann", "One")
        },
    ]);
    let folder = library
        .create_playlist("Folder", PlaylistKind::Folder, None)
        .unwrap();
    let playlist = library
        .create_playlist("Scoped", PlaylistKind::Regular, Some(folder.id))
        .unwrap();
    library
        .add_to_playlist(playlist.id, &[1, 2, 1, 3, 4, 5, 6], None)
        .unwrap();
    let query = TrackQuery {
        playlist: Some(playlist.id),
        search: Some("needle".into()),
        sort: SortKey::PlaylistOrder,
        artist_group: Some("ann".into()),
        album_group: Some("ann\u{1f}one".into()),
        genre: Some("ROCK".into()),
        ..TrackQuery::default()
    };
    assert_eq!(ids(&library, &query), [1, 1]);
    assert_eq!(library.stats(&query).unwrap().count, 2);
    assert_eq!(library.stats(&query).unwrap().total_duration_ms, 20_000);
    assert_eq!(library.albums(&query).unwrap()[0].track_count, 1);
    let facets = library.browser_facets(&query).unwrap();
    assert_eq!(
        facets.genres.len(),
        2,
        "Genre pane ignores all three browser choices"
    );
    assert_eq!(facets.artists.len(), 2, "Artist pane uses Genre only");
    assert_eq!(facets.albums.len(), 2, "Album pane uses Genre and Artist");
    assert!(
        facets
            .artists
            .iter()
            .all(|artist| artist.artwork_track_ids.is_empty()),
        "Browser facets skip artwork sampling"
    );
    let smart = library
        .create_playlist("Smart scope", PlaylistKind::Smart, None)
        .unwrap();
    assert_eq!(
        ids(
            &library,
            &TrackQuery {
                playlist: Some(smart.id),
                ..query.clone()
            }
        ),
        [1],
        "Smart membership composes with exact browser filters"
    );
    assert_eq!(
        ids(
            &library,
            &TrackQuery {
                composer: Some("BÁCH".into()),
                ..query.clone()
            }
        ),
        [1, 1]
    );
    assert!(
        ids(
            &library,
            &TrackQuery {
                ids: Some(vec![4]),
                ..query.clone()
            }
        )
        .is_empty()
    );
    assert_eq!(
        ids(
            &library,
            &TrackQuery {
                playlist: Some(folder.id),
                ..query.clone()
            }
        ),
        [1],
        "Folder membership is a union"
    );
    let scoped = library.artist_albums_in("ann", &query).unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].track_count, 1);
}

#[test]
fn scoped_artist_albums_are_chronological_and_cover_samples_are_bounded() {
    let mut rows = Vec::new();
    for i in 1..=8 {
        rows.push(Track {
            year: Some(2000 - i as i32),
            ..row(i, &format!("Song {i}"), "Ann", &format!("Album {i}"))
        });
    }
    rows.push(row(9, "Unknown year", "Ann", "Unknown"));
    let mut library = library_of(&rows);
    let playlist = library
        .create_playlist("Two albums", PlaylistKind::Regular, None)
        .unwrap();
    library.add_to_playlist(playlist.id, &[1, 3], None).unwrap();
    let query = TrackQuery {
        playlist: Some(playlist.id),
        ..TrackQuery::default()
    };
    let albums = library.artist_albums_in("ann", &query).unwrap();
    assert_eq!(
        albums.iter().map(|a| a.year).collect::<Vec<_>>(),
        [Some(1997), Some(1999)]
    );
    assert_eq!(
        library.artists(&TrackQuery::default()).unwrap()[0]
            .artwork_track_ids
            .len(),
        4
    );
    let covers = &library.artists(&query).unwrap()[0].artwork_track_ids;
    assert_eq!(covers.len(), 2);
    assert!(covers.iter().all(|id| [1, 3].contains(id)));
    assert_eq!(
        library.artist_albums("ann").unwrap().last().unwrap().year,
        None
    );
}
