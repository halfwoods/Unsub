use rusqlite::Connection;
use tempfile::TempDir;
use unsub_library::{
    DateUnit, Error, Library, LimitOrder, LimitUnit, MediaKind, Playlist, PlaylistKind, SmartField,
    SmartLimit, SmartOp, SmartRule, SmartRules, SmartValue, SortKey, Track, TrackQuery,
};

use crate::common::{library_of, rewind_schema, track};

/// Tracks 1..=5 are titled "a".."e", so playlist contents read as letters.
fn lettered() -> Library {
    let tracks: Vec<Track> = ["a", "b", "c", "d", "e"]
        .iter()
        .enumerate()
        .map(|(i, title)| {
            track(
                i as i64 + 1,
                title,
                &format!("Artist {}", 5 - i),
                "Album",
                (i as u64 + 1) * 10_000,
            )
        })
        .collect();
    library_of(&tracks)
}

fn in_order(library: &Library, playlist: i64) -> String {
    library
        .tracks(&TrackQuery {
            kind: None,
            playlist: Some(playlist),
            sort: SortKey::PlaylistOrder,
            ..TrackQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect()
}

fn regular(library: &mut Library, name: &str, items: &[i64]) -> i64 {
    let id = library
        .create_playlist(name, PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library.add_to_playlist(id, items, None).unwrap();
    id
}

fn user_playlists(library: &Library) -> Vec<Playlist> {
    library
        .playlists()
        .unwrap()
        .into_iter()
        .skip(STOCK_PLAYLISTS)
        .collect()
}

fn names(playlists: &[Playlist]) -> Vec<&str> {
    playlists.iter().map(|p| p.name.as_str()).collect()
}

const STOCK_PLAYLISTS: usize = 6;

#[test]
fn playlists_are_listed_parents_first_in_position_order() {
    let mut library = lettered();
    let first = library
        .create_playlist("First", PlaylistKind::Regular, None)
        .unwrap();
    let folder = library
        .create_playlist("Folder", PlaylistKind::Folder, None)
        .unwrap();
    let last = library
        .create_playlist("Last", PlaylistKind::Regular, None)
        .unwrap();
    let child = library
        .create_playlist("Child", PlaylistKind::Regular, Some(folder.id))
        .unwrap();
    let smart = library
        .create_playlist("Smart child", PlaylistKind::Smart, Some(folder.id))
        .unwrap();
    let inner = library
        .create_playlist("Inner", PlaylistKind::Folder, Some(folder.id))
        .unwrap();
    let deep = library
        .create_playlist("Deep", PlaylistKind::Regular, Some(inner.id))
        .unwrap();

    let listed = user_playlists(&library);

    assert_eq!(
        names(&listed),
        [
            "First",
            "Folder",
            "Child",
            "Smart child",
            "Inner",
            "Deep",
            "Last"
        ]
    );
    assert_eq!(
        listed
            .iter()
            .map(|p| (p.parent_id, p.position))
            .collect::<Vec<_>>(),
        [
            (None, 6),
            (None, 7),
            (Some(folder.id), 0),
            (Some(folder.id), 1),
            (Some(folder.id), 2),
            (Some(inner.id), 0),
            (None, 8),
        ]
    );
    assert_eq!(
        (first.kind, folder.kind, smart.kind),
        (
            PlaylistKind::Regular,
            PlaylistKind::Folder,
            PlaylistKind::Smart
        )
    );
    assert_eq!(
        (last.id, child.id, deep.id),
        (listed[6].id, listed[2].id, listed[5].id)
    );
}

#[test]
fn smart_playlists_start_with_default_rules_and_others_without() {
    let mut library = lettered();
    let smart = library
        .create_playlist("S", PlaylistKind::Smart, None)
        .unwrap();
    let plain = library
        .create_playlist("P", PlaylistKind::Regular, None)
        .unwrap();

    assert_eq!(smart.smart, Some(SmartRules::default()));
    assert_eq!(plain.smart, None);
    assert_eq!(smart.track_count, 5, "no rules means every song");
    assert_eq!(plain.track_count, 0);
}

#[test]
fn playlists_must_live_in_folders() {
    let mut library = lettered();
    let regular = library
        .create_playlist("R", PlaylistKind::Regular, None)
        .unwrap();

    let in_regular = library.create_playlist("X", PlaylistKind::Regular, Some(regular.id));
    let in_missing = library.create_playlist("X", PlaylistKind::Regular, Some(9_999));

    assert!(matches!(in_regular, Err(Error::NotAFolder(id)) if id == regular.id));
    assert!(matches!(in_missing, Err(Error::PlaylistNotFound(9_999))));
    assert_eq!(user_playlists(&library).len(), 1);
}

#[test]
fn rename_and_unknown_ids() {
    let mut library = lettered();
    let id = regular(&mut library, "Old", &[1]);

    library.rename_playlist(id, "New").unwrap();

    assert_eq!(names(&user_playlists(&library)), ["New"]);
    assert!(matches!(
        library.rename_playlist(9_999, "x"),
        Err(Error::PlaylistNotFound(9_999))
    ));
    assert!(matches!(
        library.delete_playlist(9_999),
        Err(Error::PlaylistNotFound(9_999))
    ));
    assert!(matches!(
        library.add_to_playlist(9_999, &[1], None),
        Err(Error::PlaylistNotFound(9_999))
    ));
    assert!(matches!(
        library.move_playlist(9_999, None, 0),
        Err(Error::PlaylistNotFound(9_999))
    ));
    let listing = library.tracks(&TrackQuery {
        playlist: Some(9_999),
        ..TrackQuery::default()
    });
    assert!(matches!(listing, Err(Error::PlaylistNotFound(9_999))));
}

#[test]
fn deleting_a_folder_removes_its_subtree_and_items_and_closes_the_gap() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let mut library = Library::open(&db_path).unwrap();
    library
        .import_tracks(&[track(1, "a", "x", "y", 1), track(2, "b", "x", "y", 1)])
        .unwrap();
    let before = regular(&mut library, "Before", &[1]);
    let folder = library
        .create_playlist("Folder", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    let inner = library
        .create_playlist("Inner", PlaylistKind::Folder, Some(folder))
        .unwrap()
        .id;
    let deep = library
        .create_playlist("Deep", PlaylistKind::Regular, Some(inner))
        .unwrap()
        .id;
    library.add_to_playlist(deep, &[1, 2], None).unwrap();
    let child = library
        .create_playlist("Child", PlaylistKind::Regular, Some(folder))
        .unwrap()
        .id;
    library.add_to_playlist(child, &[2], None).unwrap();
    let after = regular(&mut library, "After", &[2]);

    library.delete_playlist(folder).unwrap();

    let remaining = user_playlists(&library);
    assert_eq!(names(&remaining), ["Before", "After"]);
    assert_eq!(
        remaining.iter().map(|p| p.position).collect::<Vec<_>>(),
        [6, 7]
    );
    assert_eq!((remaining[0].id, remaining[1].id), (before, after));
    let counts = |table: &str| -> i64 {
        Connection::open(&db_path)
            .unwrap()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(
        counts("playlist_items"),
        2,
        "only Before's and After's items remain"
    );
    for gone in [folder, inner, deep, child] {
        let listing = library.tracks(&TrackQuery {
            playlist: Some(gone),
            ..TrackQuery::default()
        });
        assert!(matches!(listing, Err(Error::PlaylistNotFound(_))), "{gone}");
    }
}

#[test]
fn moving_playlists_reorders_and_reparents() {
    let mut library = lettered();
    let folder = library
        .create_playlist("F", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    let kids: Vec<i64> = ["A", "B", "C"]
        .iter()
        .map(|n| {
            library
                .create_playlist(n, PlaylistKind::Regular, Some(folder))
                .unwrap()
                .id
        })
        .collect();
    let children_of_folder = |library: &Library| -> Vec<(String, u32)> {
        library
            .playlists()
            .unwrap()
            .into_iter()
            .filter(|p| p.parent_id == Some(folder))
            .map(|p| (p.name, p.position))
            .collect()
    };

    library.move_playlist(kids[2], Some(folder), 0).unwrap();
    assert_eq!(
        children_of_folder(&library),
        [("C".into(), 0), ("A".into(), 1), ("B".into(), 2)]
    );
    library.move_playlist(kids[1], Some(folder), 99).unwrap();
    library.move_playlist(kids[0], Some(folder), 1).unwrap();
    assert_eq!(
        children_of_folder(&library),
        [("C".into(), 0), ("A".into(), 1), ("B".into(), 2)]
    );

    // Out of the folder and to the very front of the root list.
    library.move_playlist(kids[2], None, 0).unwrap();
    assert_eq!(
        children_of_folder(&library),
        [("A".into(), 0), ("B".into(), 1)],
        "the old siblings close up"
    );
    let all = library.playlists().unwrap();
    assert_eq!(
        (all[0].name.as_str(), all[0].parent_id, all[0].position),
        ("C", None, 0)
    );
    assert_eq!(all[1].position, 1);

    // Into a folder from the root.
    library.move_playlist(kids[2], Some(folder), 1).unwrap();
    assert_eq!(
        children_of_folder(&library),
        [("A".into(), 0), ("C".into(), 1), ("B".into(), 2)]
    );
}

#[test]
fn moves_that_would_create_a_cycle_or_use_a_non_folder_parent_are_rejected() {
    let mut library = lettered();
    let outer = library
        .create_playlist("Outer", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    let inner = library
        .create_playlist("Inner", PlaylistKind::Folder, Some(outer))
        .unwrap()
        .id;
    let deep = library
        .create_playlist("Deep", PlaylistKind::Folder, Some(inner))
        .unwrap()
        .id;
    let plain = library
        .create_playlist("Plain", PlaylistKind::Regular, None)
        .unwrap()
        .id;

    for target in [outer, inner, deep] {
        let moved = library.move_playlist(outer, Some(target), 0);
        assert!(
            matches!(moved, Err(Error::PlaylistCycle)),
            "outer into {target}: {moved:?}"
        );
    }
    let into_descendant = library.move_playlist(inner, Some(deep), 0);
    assert!(matches!(into_descendant, Err(Error::PlaylistCycle)));
    let into_plain = library.move_playlist(inner, Some(plain), 0);
    assert!(matches!(into_plain, Err(Error::NotAFolder(id)) if id == plain));
    let into_missing = library.move_playlist(inner, Some(9_999), 0);
    assert!(matches!(into_missing, Err(Error::PlaylistNotFound(9_999))));
    let listed = user_playlists(&library);
    assert_eq!(
        names(&listed),
        ["Outer", "Inner", "Deep", "Plain"],
        "nothing moved"
    );
}

#[test]
fn adding_tracks_appends_or_inserts_and_allows_repeats() {
    let mut library = lettered();
    let id = library
        .create_playlist("P", PlaylistKind::Regular, None)
        .unwrap()
        .id;

    library.add_to_playlist(id, &[1, 2], None).unwrap();
    library.add_to_playlist(id, &[1], None).unwrap();
    assert_eq!(in_order(&library, id), "aba");
    library.add_to_playlist(id, &[4, 5], Some(1)).unwrap();
    assert_eq!(in_order(&library, id), "adeba");
    library.add_to_playlist(id, &[3], Some(0)).unwrap();
    assert_eq!(in_order(&library, id), "cadeba");
    library.add_to_playlist(id, &[3], Some(99)).unwrap();
    assert_eq!(in_order(&library, id), "cadebac");
    assert_eq!(user_playlists(&library)[0].track_count, 7, "repeats count");
}

#[test]
fn adding_unknown_tracks_or_to_the_wrong_kind_of_playlist_changes_nothing() {
    let mut library = lettered();
    let id = regular(&mut library, "P", &[1]);
    let smart = library
        .create_playlist("S", PlaylistKind::Smart, None)
        .unwrap()
        .id;
    let folder = library
        .create_playlist("F", PlaylistKind::Folder, None)
        .unwrap()
        .id;

    let unknown = library.add_to_playlist(id, &[2, 9_999], None);
    assert!(
        matches!(unknown, Err(Error::TrackNotFound(9_999))),
        "{unknown:?}"
    );
    assert_eq!(in_order(&library, id), "a");
    for wrong in [smart, folder] {
        assert!(matches!(
            library.add_to_playlist(wrong, &[1], None),
            Err(Error::NotARegularPlaylist(_))
        ));
        assert!(matches!(
            library.remove_from_playlist(wrong, &[0]),
            Err(Error::NotARegularPlaylist(_))
        ));
        assert!(matches!(
            library.move_in_playlist(wrong, &[0], 0),
            Err(Error::NotARegularPlaylist(_))
        ));
    }
    assert!(matches!(
        library.set_smart_rules(id, &SmartRules::default()),
        Err(Error::NotASmartPlaylist(_))
    ));
}

#[test]
fn removing_by_position_takes_only_those_rows() {
    let mut library = lettered();
    let id = regular(&mut library, "P", &[1, 2, 1, 3]);

    library.remove_from_playlist(id, &[2, 2, 99]).unwrap();
    assert_eq!(
        in_order(&library, id),
        "abc",
        "the repeat at index 2 went, not the first one"
    );
    library.remove_from_playlist(id, &[]).unwrap();
    library.remove_from_playlist(id, &[7]).unwrap();
    assert_eq!(in_order(&library, id), "abc");
    library.remove_from_playlist(id, &[0, 2]).unwrap();
    assert_eq!(in_order(&library, id), "b");
}

#[test]
fn moving_rows_within_a_playlist_drops_them_before_the_target_row() {
    let cases: [(&[u32], u32, &str); 9] = [
        (&[1, 3], 0, "bdace"),
        (&[1, 3], 2, "abdce"),
        (&[1, 3], 5, "acebd"),
        (&[1, 3], 99, "acebd"),
        (&[0], 99, "bcdea"),
        (&[4], 0, "eabcd"),
        (&[3, 1, 1, 9], 0, "bdace"),
        (&[], 2, "abcde"),
        (&[9], 0, "abcde"),
    ];
    for (positions, to, expected) in cases {
        let mut library = lettered();
        let id = regular(&mut library, "P", &[1, 2, 3, 4, 5]);

        library.move_in_playlist(id, positions, to).unwrap();

        assert_eq!(
            in_order(&library, id),
            expected,
            "move {positions:?} to {to}"
        );
    }
}

#[test]
fn a_playlist_lists_its_rows_in_order_with_repeats_and_sorts_like_any_list() {
    let mut library = lettered();
    let id = regular(&mut library, "P", &[3, 1, 3, 2]);
    let query = |sort, descending| TrackQuery {
        kind: None,
        playlist: Some(id),
        sort,
        descending,
        ..TrackQuery::default()
    };
    let titles = |query: &TrackQuery| -> String {
        library
            .tracks(query)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    };

    assert_eq!(titles(&query(SortKey::PlaylistOrder, false)), "cacb");
    assert_eq!(titles(&query(SortKey::PlaylistOrder, true)), "bcac");
    assert_eq!(
        titles(&query(SortKey::Name, false)),
        "abcc",
        "repeats sit together"
    );
    // Artists are numbered in reverse of the titles, so c (Artist 3) sorts after b.
    assert_eq!(titles(&query(SortKey::Artist, false)), "ccba");
    let ids: Vec<i64> = library
        .tracks(&query(SortKey::PlaylistOrder, false))
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, [3, 1, 3, 2], "rows carry the track's own id");

    let stats = library.stats(&query(SortKey::Name, false)).unwrap();
    assert_eq!(stats.count, 4);
    assert_eq!(stats.total_duration_ms, 30_000 + 10_000 + 30_000 + 20_000);
}

#[test]
fn a_playlist_listing_combines_with_search_kind_and_ids() {
    let mut library = lettered();
    let id = regular(&mut library, "P", &[3, 1, 3, 2]);
    let mut podcast = track(6, "f", "Artist 0", "Album", 1);
    podcast.kind = MediaKind::Podcast;
    library.import_tracks(&[podcast]).unwrap();
    library.add_to_playlist(id, &[6], None).unwrap();
    let base = || TrackQuery {
        kind: None,
        playlist: Some(id),
        sort: SortKey::PlaylistOrder,
        ..TrackQuery::default()
    };
    let list = |query: TrackQuery| -> String {
        library
            .tracks(&query)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    };

    assert_eq!(list(base()), "cacbf");
    assert_eq!(
        list(TrackQuery {
            kind: Some(MediaKind::Music),
            ..base()
        }),
        "cacb"
    );
    assert_eq!(
        list(TrackQuery {
            kind: Some(MediaKind::Podcast),
            ..base()
        }),
        "f"
    );
    assert_eq!(
        list(TrackQuery {
            search: Some("artist 3".into()),
            ..base()
        }),
        "cc"
    );
    assert_eq!(
        list(TrackQuery {
            ids: Some(vec![1, 2]),
            ..base()
        }),
        "ab"
    );
    assert_eq!(
        list(TrackQuery {
            ids: Some(vec![]),
            ..base()
        }),
        ""
    );
    let narrowed = TrackQuery {
        kind: Some(MediaKind::Music),
        search: Some("artist".into()),
        ids: Some(vec![3, 6]),
        ..base()
    };
    assert_eq!(list(narrowed), "cc");
}

#[test]
fn playlist_order_outside_a_playlist_falls_back_to_artist_order() {
    let library = lettered();
    let titles = |sort| -> String {
        library
            .tracks(&TrackQuery {
                sort,
                ..TrackQuery::default()
            })
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    };

    assert_eq!(titles(SortKey::PlaylistOrder), titles(SortKey::Artist));
    assert_eq!(titles(SortKey::PlaylistOrder), "edcba");
}

#[test]
fn deleting_tracks_removes_them_from_playlists_and_indexes_follow_the_listing() {
    let mut library = lettered();
    let id = regular(&mut library, "P", &[1, 2, 3, 2, 4]);

    library.remove_tracks(&[2]).unwrap();

    assert_eq!(in_order(&library, id), "acd");
    assert_eq!(user_playlists(&library)[0].track_count, 3);
    library.remove_from_playlist(id, &[1]).unwrap();
    assert_eq!(
        in_order(&library, id),
        "ad",
        "index 1 of the listing, whatever was stored"
    );
    library.move_in_playlist(id, &[1], 0).unwrap();
    assert_eq!(in_order(&library, id), "da");
}

#[test]
fn a_folder_lists_the_union_of_its_subtree_once_per_track() {
    let mut library = lettered();
    let folder = library
        .create_playlist("F", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    let one = library
        .create_playlist("One", PlaylistKind::Regular, Some(folder))
        .unwrap()
        .id;
    library.add_to_playlist(one, &[1, 2, 2], None).unwrap();
    let two = library
        .create_playlist("Two", PlaylistKind::Regular, Some(folder))
        .unwrap()
        .id;
    library.add_to_playlist(two, &[2, 3], None).unwrap();
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
                    value: SmartValue::Text("d".into()),
                }],
                ..SmartRules::default()
            },
        )
        .unwrap();
    let nested = library
        .create_playlist("Nested", PlaylistKind::Folder, Some(folder))
        .unwrap()
        .id;
    let deep = library
        .create_playlist("Deep", PlaylistKind::Regular, Some(nested))
        .unwrap()
        .id;
    library.add_to_playlist(deep, &[3, 5], None).unwrap();
    let empty = library
        .create_playlist("Empty", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    let listing = |playlist| TrackQuery {
        kind: None,
        playlist: Some(playlist),
        sort: SortKey::Name,
        ..TrackQuery::default()
    };

    let titles: String = library
        .tracks(&listing(folder))
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect();

    assert_eq!(titles, "abcde");
    assert_eq!(library.stats(&listing(folder)).unwrap().count, 5);
    assert!(library.tracks(&listing(empty)).unwrap().is_empty());
    let by_name = |name: &str| {
        user_playlists(&library)
            .into_iter()
            .find(|p| p.name == name)
            .unwrap()
            .track_count
    };
    assert_eq!(by_name("F"), 5);
    assert_eq!(by_name("One"), 3, "a regular playlist counts its repeats");
    assert_eq!(by_name("Nested"), 2);
    assert_eq!(by_name("Empty"), 0);
}

#[test]
fn playlists_survive_reopening() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    {
        let mut library = Library::open(&db_path).unwrap();
        library
            .import_tracks(&[track(1, "a", "x", "y", 1), track(2, "b", "x", "y", 1)])
            .unwrap();
        let folder = library
            .create_playlist("F", PlaylistKind::Folder, None)
            .unwrap()
            .id;
        let id = library
            .create_playlist("P", PlaylistKind::Regular, Some(folder))
            .unwrap()
            .id;
        library.add_to_playlist(id, &[2, 1, 2], None).unwrap();
    }

    let library = Library::open(&db_path).unwrap();

    let listed = user_playlists(&library);
    assert_eq!(names(&listed), ["F", "P"]);
    assert_eq!(in_order(&library, listed[1].id), "bab");
}

#[test]
fn a_new_database_gets_itunes_stock_smart_playlists() {
    let library = Library::open_in_memory().unwrap();

    let stock = library.playlists().unwrap();

    assert_eq!(
        names(&stock),
        [
            "90\u{2019}s Music",
            "Classical Music",
            "My Top Rated",
            "Recently Added",
            "Recently Played",
            "Top 25 Most Played",
        ]
    );
    assert!(
        stock
            .iter()
            .all(|p| p.kind == PlaylistKind::Smart && p.parent_id.is_none())
    );
    assert_eq!(
        stock.iter().map(|p| p.position).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4, 5]
    );
    let rules = |i: usize| stock[i].smart.clone().unwrap();
    let one = |field, op, value| vec![SmartRule::Condition { field, op, value }];
    let two_weeks = SmartValue::Relative {
        amount: 2,
        unit: DateUnit::Weeks,
    };
    assert_eq!(
        rules(0).rules,
        one(
            SmartField::Year,
            SmartOp::InRange,
            SmartValue::Range {
                from: 1990,
                to: 1999
            }
        )
    );
    assert_eq!(
        rules(1).rules,
        one(
            SmartField::Genre,
            SmartOp::Is,
            SmartValue::Text("Classical".into())
        )
    );
    assert_eq!(
        rules(2).rules,
        one(
            SmartField::Rating,
            SmartOp::GreaterThan,
            SmartValue::Number(80)
        )
    );
    assert_eq!(
        rules(3).rules,
        one(SmartField::DateAdded, SmartOp::InTheLast, two_weeks.clone())
    );
    assert_eq!(
        rules(4).rules,
        one(SmartField::LastPlayed, SmartOp::InTheLast, two_weeks)
    );
    assert_eq!(
        rules(5).rules,
        one(
            SmartField::Plays,
            SmartOp::GreaterThan,
            SmartValue::Number(0)
        )
    );
    assert_eq!(
        rules(5).limit,
        Some(SmartLimit {
            amount: 25,
            unit: LimitUnit::Items,
            selected_by: LimitOrder::MostOftenPlayed
        })
    );
    assert!(stock.iter().all(|p| {
        let r = p.smart.as_ref().unwrap();
        r.match_all && r.live && !r.only_checked
    }));
}

#[test]
fn the_stock_playlists_pick_the_right_songs() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let day = 86_400;
    let mut tracks = vec![
        Track {
            year: Some(1990),
            date_added: now - 30 * day,
            ..track(1, "ninety", "x", "y", 1)
        },
        Track {
            year: Some(1999),
            date_added: now - 30 * day,
            ..track(2, "ninety-nine", "x", "y", 1)
        },
        Track {
            year: Some(2000),
            date_added: now - 30 * day,
            ..track(3, "naughty", "x", "y", 1)
        },
        Track {
            genre: "classical".into(),
            date_added: now - 30 * day,
            ..track(4, "sonata", "x", "y", 1)
        },
        Track {
            rating: 100,
            date_added: now - 30 * day,
            ..track(5, "five stars", "x", "y", 1)
        },
        Track {
            rating: 80,
            date_added: now - 30 * day,
            ..track(6, "four stars", "x", "y", 1)
        },
        Track {
            date_added: now - day,
            ..track(7, "new", "x", "y", 1)
        },
        Track {
            last_played: Some(now - 3 * day),
            date_added: now - 30 * day,
            ..track(8, "played", "x", "y", 1)
        },
    ];
    for n in 0..30 {
        tracks.push(Track {
            play_count: 1 + n as u32,
            date_added: now - 30 * day,
            ..track(100 + n, &format!("hit {n:02}"), "x", "y", 1)
        });
    }
    let library = library_of(&tracks);
    let list = |name: &str| -> Vec<String> {
        let id = library
            .playlists()
            .unwrap()
            .into_iter()
            .find(|p| p.name == name)
            .unwrap()
            .id;
        library
            .tracks(&TrackQuery {
                kind: None,
                playlist: Some(id),
                sort: SortKey::Name,
                ..TrackQuery::default()
            })
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    };

    assert_eq!(list("90\u{2019}s Music"), ["ninety", "ninety-nine"]);
    assert_eq!(list("Classical Music"), ["sonata"]);
    assert_eq!(list("My Top Rated"), ["five stars"]);
    assert_eq!(list("Recently Added"), ["new"]);
    assert_eq!(list("Recently Played"), ["played"]);
    let top = list("Top 25 Most Played");
    assert_eq!(top.len(), 25);
    assert!(top.contains(&"hit 29".to_owned()) && top.contains(&"hit 05".to_owned()));
    assert!(
        !top.contains(&"hit 04".to_owned()),
        "the 25 most played only"
    );
}

#[test]
fn stock_playlists_are_only_written_when_the_database_is_created() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    {
        let mut library = Library::open(&db_path).unwrap();
        assert_eq!(library.playlists().unwrap().len(), STOCK_PLAYLISTS);
        for playlist in library.playlists().unwrap() {
            library.delete_playlist(playlist.id).unwrap();
        }
    }
    assert!(
        Library::open(&db_path)
            .unwrap()
            .playlists()
            .unwrap()
            .is_empty(),
        "reopening does not reseed"
    );

    // An older database upgraded by this build keeps whatever it had: none.
    rewind_schema(&db_path, 2);
    let upgraded = Library::open(&db_path).unwrap();
    assert!(upgraded.playlists().unwrap().is_empty());
}

#[test]
fn json_contract_for_playlists_and_playlist_queries() {
    let mut library = lettered();
    let folder = library
        .create_playlist("F", PlaylistKind::Folder, None)
        .unwrap();
    let smart = library
        .create_playlist("S", PlaylistKind::Smart, Some(folder.id))
        .unwrap();

    let json = serde_json::to_value(&smart).unwrap();

    assert_eq!(json["kind"], "smart");
    assert_eq!(json["parentId"], folder.id);
    assert_eq!(json["trackCount"], 5);
    assert_eq!(json["smart"]["matchAll"], true);
    assert_eq!(
        serde_json::to_value(&folder).unwrap()["smart"],
        serde_json::Value::Null
    );

    let query: TrackQuery =
        serde_json::from_str(r#"{"playlist":7,"sort":"playlistOrder","descending":true}"#).unwrap();
    assert_eq!(
        (query.playlist, query.sort, query.descending),
        (Some(7), SortKey::PlaylistOrder, true)
    );
    assert_eq!(TrackQuery::default().playlist, None);
}

#[test]
fn set_playlist_items_replaces_the_contents_in_order_with_repeats() {
    let mut library = lettered();
    let id = library
        .create_playlist("Mix", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library.add_to_playlist(id, &[1, 2, 3], None).unwrap();

    library.set_playlist_items(id, &[5, 4, 5]).unwrap();
    assert_eq!(in_order(&library, id), "ede");

    library.set_playlist_items(id, &[]).unwrap();
    assert_eq!(in_order(&library, id), "");
}

#[test]
fn set_playlist_items_is_all_or_nothing_and_only_for_regular_playlists() {
    let mut library = lettered();
    let id = library
        .create_playlist("Mix", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    let folder = library
        .create_playlist("Folder", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    library.add_to_playlist(id, &[1, 2], None).unwrap();

    assert!(matches!(
        library.set_playlist_items(id, &[3, 9_999]),
        Err(Error::TrackNotFound(9_999))
    ));
    assert_eq!(in_order(&library, id), "ab");
    assert!(matches!(
        library.set_playlist_items(folder, &[1]),
        Err(Error::NotARegularPlaylist(_))
    ));
}

#[test]
fn a_deleted_playlists_id_is_never_given_to_another() {
    let mut library = lettered();
    let first = library
        .create_playlist("One", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    let second = library
        .create_playlist("Two", PlaylistKind::Folder, None)
        .unwrap()
        .id;

    library.delete_playlist(second).unwrap();
    let third = library
        .create_playlist("Three", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library.delete_playlist(first).unwrap();
    let fourth = library
        .create_playlist("Four", PlaylistKind::Smart, None)
        .unwrap()
        .id;

    assert!(third > second, "{third} after {second}");
    assert!(fourth > third, "{fourth} after {third}");
}

#[test]
fn ids_stay_unused_across_reopening_and_for_the_stock_playlists_too() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let highest = {
        let mut library = Library::open(&db_path).unwrap();
        // Deleting the last of the stock playlists would free its id for the next one made.
        let stock = library.playlists().unwrap();
        let last = stock.iter().map(|p| p.id).max().unwrap();
        library.delete_playlist(last).unwrap();
        last
    };

    let mut library = Library::open(&db_path).unwrap();
    let made = library
        .create_playlist("New", PlaylistKind::Regular, None)
        .unwrap();

    assert!(made.id > highest, "{} after {highest}", made.id);
}

#[test]
fn a_database_from_before_the_floor_starts_it_at_its_highest_playlist() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let highest = {
        let mut library = Library::open(&db_path).unwrap();
        library
            .create_playlist("Mine", PlaylistKind::Regular, None)
            .unwrap()
            .id
    };
    rewind_schema(&db_path, 10);
    Connection::open(&db_path)
        .unwrap()
        .execute("DELETE FROM settings WHERE key = 'playlist_id_floor'", [])
        .unwrap();

    let mut library = Library::open(&db_path).unwrap();
    library.delete_playlist(highest).unwrap();
    let made = library
        .create_playlist("Next", PlaylistKind::Regular, None)
        .unwrap();

    assert!(made.id > highest, "{} after {highest}", made.id);
}

#[test]
fn a_playlist_exports_as_m3u8_in_playlist_order_with_repeats() {
    let mut library = lettered();
    let id = regular(&mut library, "Mix", &[3, 1, 3]);

    assert_eq!(
        library.playlist_m3u8(id).unwrap(),
        "#EXTM3U\n\
         #EXTINF:30,Artist 3 - c\n/music/3.mp3\n\
         #EXTINF:10,Artist 5 - a\n/music/1.mp3\n\
         #EXTINF:30,Artist 3 - c\n/music/3.mp3\n"
    );
}

#[test]
fn an_m3u8_export_rounds_seconds_and_names_what_is_tagged() {
    let mut library = library_of(&[
        track(1, "Title Only", "", "Album", 1_499),
        Track {
            title: String::new(),
            ..track(2, "", "Artist Only", "Album", 1_500)
        },
        Track {
            title: "Two\nLines".into(),
            ..track(3, "", "Artist", "Album", 0)
        },
    ]);
    let id = regular(&mut library, "Odd tags", &[1, 2, 3]);

    assert_eq!(
        library.playlist_m3u8(id).unwrap(),
        "#EXTM3U\n\
         #EXTINF:1,Title Only\n/music/1.mp3\n\
         #EXTINF:2,Artist Only\n/music/2.mp3\n\
         #EXTINF:-1,Artist - Two Lines\n/music/3.mp3\n"
    );
}

#[test]
fn an_m3u8_export_skips_stations_but_keeps_cloud_and_missing_songs() {
    let mut library = library_of(&[
        track(1, "File", "Artist", "Album", 60_000),
        Track {
            path: "https://radio.example/stream".into(),
            kind: MediaKind::Radio,
            ..track(2, "Station", "", "", 0)
        },
        Track {
            in_cloud: true,
            ..track(3, "Placeholder", "Artist", "Album", 60_000)
        },
        Track {
            missing: true,
            ..track(4, "Gone", "Artist", "Album", 60_000)
        },
    ]);
    let id = regular(&mut library, "Mixed", &[1, 2, 3, 4]);

    let text = library.playlist_m3u8(id).unwrap();

    assert_eq!(text.matches("#EXTINF").count(), 3);
    assert!(!text.contains("radio.example"));
    assert!(text.contains("/music/3.mp3\n") && text.contains("/music/4.mp3\n"));
}

#[test]
fn a_duplicate_keeps_a_regular_playlists_order_with_repeats_beside_it() {
    let mut library = lettered();
    let folder = library
        .create_playlist("Folder", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    let original = library
        .create_playlist("Mix", PlaylistKind::Regular, Some(folder))
        .unwrap()
        .id;
    library
        .add_to_playlist(original, &[3, 1, 3, 2], None)
        .unwrap();
    let after = library
        .create_playlist("After", PlaylistKind::Regular, Some(folder))
        .unwrap()
        .id;

    let copy = library.duplicate_playlist(original).unwrap();
    let again = library.duplicate_playlist(original).unwrap();

    assert_eq!(in_order(&library, copy), "cacb");
    assert_eq!(in_order(&library, original), "cacb");
    let listed = library.playlists().unwrap();
    let ids: Vec<i64> = listed.iter().skip(STOCK_PLAYLISTS).map(|p| p.id).collect();
    assert_eq!(ids, [folder, original, again, copy, after]);
    let copied = listed.iter().find(|p| p.id == copy).unwrap();
    assert_eq!(
        (copied.name.as_str(), copied.parent_id, copied.kind),
        ("Mix 2", Some(folder), PlaylistKind::Regular)
    );
    assert_eq!(listed.iter().find(|p| p.id == again).unwrap().name, "Mix 3");
}

#[test]
fn a_duplicate_of_a_frozen_smart_playlist_keeps_its_exact_songs_and_rules() {
    let mut library = lettered();
    let original = library
        .create_playlist("Picks", PlaylistKind::Smart, None)
        .unwrap()
        .id;
    let rules = SmartRules {
        limit: Some(SmartLimit {
            amount: 3,
            unit: LimitUnit::Items,
            selected_by: LimitOrder::Random,
        }),
        live: false,
        ..SmartRules::default()
    };
    library.set_smart_rules(original, &rules).unwrap();

    let copy = library.duplicate_playlist(original).unwrap();

    assert_eq!(in_order(&library, copy).len(), 3);
    assert_eq!(in_order(&library, copy), in_order(&library, original));
    let copied = library
        .playlists()
        .unwrap()
        .into_iter()
        .find(|p| p.id == copy)
        .unwrap();
    assert_eq!(copied.smart, Some(rules));
    assert_eq!(copied.track_count, 3);
}

#[test]
fn a_folder_or_a_missing_playlist_cannot_be_duplicated() {
    let mut library = lettered();
    let folder = library
        .create_playlist("Folder", PlaylistKind::Folder, None)
        .unwrap()
        .id;

    assert!(matches!(
        library.duplicate_playlist(folder),
        Err(Error::CannotDuplicateFolder(id)) if id == folder
    ));
    assert!(matches!(
        library.duplicate_playlist(9_999),
        Err(Error::PlaylistNotFound(9_999))
    ));
}

#[test]
fn exporting_a_playlist_that_does_not_exist_fails() {
    let library = lettered();

    assert!(matches!(
        library.playlist_m3u8(9_999),
        Err(Error::PlaylistNotFound(9_999))
    ));
}
