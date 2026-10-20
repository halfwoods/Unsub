use std::fs;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::ItemKey;
use unsub_library::{Library, MediaKind, SortKey, TrackQuery};

use crate::common::{
    MUSIC_COUNT, all_kinds, copy_fixtures, fixtures_dir, scan, scanned_library, titles,
    track_titled,
};

fn sorted(sort: SortKey, descending: bool) -> TrackQuery {
    TrackQuery {
        sort,
        descending,
        ..TrackQuery::default()
    }
}

#[test]
fn default_query_is_music_by_artist_with_the_stripped_and_blanks_last() {
    let (library, _dir) = scanned_library();

    // Beatles, Beyoncé, (The) Cranberries, Queen, then the untagged file.
    assert_eq!(
        titles(&library, &TrackQuery::default()),
        [
            "Come Together",
            "Something",
            "Halo",
            "Zombie",
            "Bohemian Rhapsody",
            "untagged"
        ]
    );
}

#[test]
fn descending_reverses_only_the_primary_key() {
    let (library, _dir) = scanned_library();

    // Artists run Queen..Beatles, but the Beatles' tracks stay in track order
    // and the blank artist stays last.
    assert_eq!(
        titles(&library, &sorted(SortKey::Artist, true)),
        [
            "Bohemian Rhapsody",
            "Zombie",
            "Halo",
            "Come Together",
            "Something",
            "untagged"
        ]
    );
}

#[test]
fn album_sort_ignores_leading_articles() {
    let (library, _dir) = scanned_library();

    // "A Night at the Opera" files under N, between "I Am..." and "No Need".
    assert_eq!(
        titles(&library, &sorted(SortKey::Album, false)),
        [
            "Come Together",
            "Something",
            "Halo",
            "Bohemian Rhapsody",
            "Zombie",
            "untagged"
        ]
    );
}

#[test]
fn album_artist_sort_falls_back_to_track_artist() {
    let (library, _dir) = scanned_library();

    // Zombie has no album artist, so it files under "Cranberries".
    assert_eq!(
        titles(&library, &sorted(SortKey::AlbumArtist, false)),
        [
            "Come Together",
            "Something",
            "Halo",
            "Zombie",
            "Bohemian Rhapsody",
            "untagged"
        ]
    );
}

#[test]
fn name_sort_is_case_insensitive_with_blanks_last() {
    let (library, _dir) = scanned_library();

    assert_eq!(
        titles(&library, &sorted(SortKey::Name, false)),
        [
            "Bohemian Rhapsody",
            "Come Together",
            "Halo",
            "Something",
            "untagged",
            "Zombie"
        ]
    );
    assert_eq!(
        titles(&library, &sorted(SortKey::Name, true)),
        [
            "Zombie",
            "untagged",
            "Something",
            "Halo",
            "Come Together",
            "Bohemian Rhapsody"
        ]
    );
}

#[test]
fn genre_sort_breaks_ties_by_artist_then_track() {
    let (library, _dir) = scanned_library();

    assert_eq!(
        titles(&library, &sorted(SortKey::Genre, false)),
        [
            "Zombie",
            "Halo",
            "Come Together",
            "Something",
            "Bohemian Rhapsody",
            "untagged"
        ]
    );
}

#[test]
fn numeric_sorts_put_missing_values_last_in_both_directions() {
    let (library, _dir) = scanned_library();

    assert_eq!(
        titles(&library, &sorted(SortKey::Year, true)),
        [
            "Halo",
            "Zombie",
            "Bohemian Rhapsody",
            "Come Together",
            "Something",
            "untagged"
        ]
    );
    assert_eq!(
        titles(&library, &sorted(SortKey::Year, false)),
        [
            "Come Together",
            "Something",
            "Bohemian Rhapsody",
            "Zombie",
            "Halo",
            "untagged"
        ]
    );
}

#[test]
fn every_sort_key_yields_all_tracks() {
    let (library, _dir) = scanned_library();
    let keys = [
        SortKey::Name,
        SortKey::Time,
        SortKey::Artist,
        SortKey::Album,
        SortKey::AlbumArtist,
        SortKey::Genre,
        SortKey::Year,
        SortKey::Rating,
        SortKey::Plays,
        SortKey::LastPlayed,
        SortKey::DateAdded,
        SortKey::Kind,
        SortKey::Bitrate,
        SortKey::SampleRate,
        SortKey::Size,
        SortKey::PlaylistOrder,
    ];

    for key in keys {
        for descending in [false, true] {
            let count = library.tracks(&sorted(key, descending)).unwrap().len();
            assert_eq!(count, MUSIC_COUNT, "{key:?} descending={descending}");
        }
    }
}

#[test]
fn rating_play_and_size_sorts_use_stored_values() {
    let (mut library, _dir) = scanned_library();
    let zombie = track_titled(&library, "Zombie");
    let halo = track_titled(&library, "Halo");
    library.set_rating(zombie.id, 100).unwrap();
    library.set_rating(halo.id, 60).unwrap();
    library.record_play(halo.id, 10).unwrap();
    library.record_play(halo.id, 20).unwrap();
    library.record_play(zombie.id, 30).unwrap();

    let by_rating = titles(&library, &sorted(SortKey::Rating, true));
    assert_eq!(&by_rating[..2], ["Zombie", "Halo"]);
    let by_plays = titles(&library, &sorted(SortKey::Plays, true));
    assert_eq!(&by_plays[..2], ["Halo", "Zombie"]);
    let by_last_played = titles(&library, &sorted(SortKey::LastPlayed, true));
    assert_eq!(&by_last_played[..2], ["Zombie", "Halo"]);
    assert_eq!(
        by_last_played.len(),
        MUSIC_COUNT,
        "never-played tracks still listed"
    );

    let sizes: Vec<u64> = library
        .tracks(&sorted(SortKey::Size, false))
        .unwrap()
        .iter()
        .map(|t| t.file_size)
        .collect();
    assert!(sizes.windows(2).all(|pair| pair[0] <= pair[1]), "{sizes:?}");
}

#[test]
fn explicit_sort_tag_overrides_the_derived_key() {
    let dir = copy_fixtures();
    let path = dir.path().join("come_together.flac");
    let mut tagged = lofty::read_from_path(&path).unwrap();
    tagged
        .primary_tag_mut()
        .unwrap()
        .insert_text(ItemKey::TrackArtistSortOrder, "Zzz Beatles, The".into());
    tagged.save_to_path(&path, WriteOptions::default()).unwrap();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    let by_artist = titles(&library, &TrackQuery::default());
    assert_eq!(by_artist[by_artist.len() - 2], "Come Together");
    assert_eq!(
        track_titled(&library, "Come Together")
            .sort_artist
            .as_deref(),
        Some("Zzz Beatles, The")
    );
}

#[test]
fn search_requires_every_word_and_ignores_case_and_diacritics() {
    let (library, _dir) = scanned_library();
    let search = |text: &str| {
        titles(
            &library,
            &TrackQuery {
                search: Some(text.into()),
                ..TrackQuery::default()
            },
        )
    };

    assert_eq!(search("beyonce"), ["Halo"]);
    assert_eq!(search("BEYONCÉ"), ["Halo"]);
    assert_eq!(search("abbey beatles"), ["Come Together", "Something"]);
    assert_eq!(
        search("  beatles   abbey  "),
        ["Come Together", "Something"]
    );
    assert_eq!(search("abbey queen"), Vec::<String>::new());
    assert_eq!(search("sasha pop"), ["Halo"]);
    assert_eq!(search("lennon"), ["Come Together"]);
    assert_eq!(search("alternative"), ["Zombie"]);
    assert_eq!(search("tle"), ["Come Together", "Something"]);
    assert_eq!(search("   ").len(), MUSIC_COUNT);
}

#[test]
fn search_treats_sql_wildcards_literally() {
    let (library, _dir) = scanned_library();
    let search = |text: &str| {
        library
            .tracks(&TrackQuery {
                search: Some(text.into()),
                ..TrackQuery::default()
            })
            .unwrap()
            .len()
    };

    assert_eq!(search("%"), 0);
    assert_eq!(search("_"), 0);
    assert_eq!(search("'; DROP TABLE tracks; --"), 0);
    assert_eq!(search("..."), 1, "the literal dots in an album title");
}

#[test]
fn kind_filter_selects_media_kinds() {
    let (library, _dir) = scanned_library();
    let of = |kind| TrackQuery {
        kind,
        ..TrackQuery::default()
    };

    assert_eq!(
        titles(&library, &of(Some(MediaKind::Podcast))),
        ["Episode 12"]
    );
    assert_eq!(
        titles(&library, &of(Some(MediaKind::Audiobook))),
        ["Chapter 1"]
    );
    assert_eq!(
        library.tracks(&of(Some(MediaKind::Music))).unwrap().len(),
        MUSIC_COUNT
    );
    assert_eq!(library.tracks(&of(None)).unwrap().len(), MUSIC_COUNT + 2);
}

#[test]
fn stats_follow_the_same_filter_as_tracks() {
    let (library, _dir) = scanned_library();
    let query = TrackQuery {
        search: Some("beatles".into()),
        ..TrackQuery::default()
    };

    let tracks = library.tracks(&query).unwrap();
    let stats = library.stats(&query).unwrap();

    assert_eq!(stats.count, 2);
    assert_eq!(
        stats.total_duration_ms,
        tracks.iter().map(|t| t.duration_ms).sum::<u64>()
    );
    assert_eq!(
        stats.total_bytes,
        tracks.iter().map(|t| t.file_size).sum::<u64>()
    );
    assert_eq!(
        stats.total_bytes,
        fs::metadata(fixtures_dir().join("come_together.flac"))
            .unwrap()
            .len()
            + fs::metadata(fixtures_dir().join("something.m4a"))
                .unwrap()
                .len()
    );
}

#[test]
fn stats_of_an_empty_library_are_zero() {
    let library = Library::open_in_memory().unwrap();

    let stats = library.stats(&all_kinds()).unwrap();

    assert_eq!(
        (stats.count, stats.total_duration_ms, stats.total_bytes),
        (0, 0, 0)
    );
}

#[test]
fn track_lookup_by_id() {
    let (library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo");

    assert_eq!(library.track(halo.id).unwrap(), Some(halo));
    assert_eq!(library.track(i64::MAX).unwrap(), None);
}

fn with_ids(library: &Library, titles_wanted: &[&str]) -> Vec<i64> {
    titles_wanted
        .iter()
        .map(|title| track_titled(library, title).id)
        .collect()
}

#[test]
fn ids_restrict_the_list_and_sort_like_any_other() {
    let (library, _dir) = scanned_library();
    let ids = Some(with_ids(
        &library,
        &["Zombie", "Episode 12", "Halo", "Come Together"],
    ));
    let query = |descending| TrackQuery {
        ids: ids.clone(),
        descending,
        ..all_kinds()
    };

    assert_eq!(
        titles(&library, &query(false)),
        ["Come Together", "Halo", "Zombie", "Episode 12"]
    );
    assert_eq!(
        titles(&library, &query(true)),
        ["Episode 12", "Zombie", "Halo", "Come Together"]
    );
}

#[test]
fn ids_combine_with_kind_and_search() {
    let (library, _dir) = scanned_library();
    let ids = Some(with_ids(
        &library,
        &["Come Together", "Something", "Halo", "Episode 12"],
    ));
    let query = |kind, search: Option<&str>| TrackQuery {
        kind,
        search: search.map(Into::into),
        ids: ids.clone(),
        ..TrackQuery::default()
    };

    assert_eq!(
        titles(&library, &query(Some(MediaKind::Music), None)),
        ["Come Together", "Something", "Halo"]
    );
    assert_eq!(
        titles(&library, &query(Some(MediaKind::Podcast), None)),
        ["Episode 12"]
    );
    assert_eq!(titles(&library, &query(None, None)).len(), 4);
    assert_eq!(
        titles(&library, &query(Some(MediaKind::Music), Some("beatles"))),
        ["Come Together", "Something"]
    );
    // Zombie matches "cranberries" but is outside the id set.
    assert!(
        titles(
            &library,
            &query(Some(MediaKind::Music), Some("cranberries"))
        )
        .is_empty()
    );
    assert!(titles(&library, &query(Some(MediaKind::Podcast), Some("beatles"))).is_empty());
}

#[test]
fn empty_ids_match_nothing_while_absent_ids_match_everything() {
    let (library, _dir) = scanned_library();
    let none = TrackQuery {
        ids: Some(vec![]),
        ..all_kinds()
    };

    assert!(library.tracks(&none).unwrap().is_empty());
    let stats = library.stats(&none).unwrap();
    assert_eq!(
        (stats.count, stats.total_duration_ms, stats.total_bytes),
        (0, 0, 0)
    );
    assert_eq!(library.tracks(&all_kinds()).unwrap().len(), MUSIC_COUNT + 2);
}

#[test]
fn unknown_and_repeated_ids_are_harmless() {
    let (library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    let query = TrackQuery {
        ids: Some(vec![halo, i64::MAX, halo, -5]),
        ..all_kinds()
    };

    assert_eq!(titles(&library, &query), ["Halo"]);
    assert_eq!(library.stats(&query).unwrap().count, 1);
}

#[test]
fn stats_follow_the_ids_filter() {
    let (library, _dir) = scanned_library();
    let wanted = ["Halo", "Zombie"];
    let query = TrackQuery {
        ids: Some(with_ids(&library, &wanted)),
        ..all_kinds()
    };

    let tracks = library.tracks(&query).unwrap();
    let stats = library.stats(&query).unwrap();

    assert_eq!(stats.count, 2);
    assert_eq!(
        stats.total_bytes,
        tracks.iter().map(|t| t.file_size).sum::<u64>()
    );
    assert_eq!(
        stats.total_duration_ms,
        tracks.iter().map(|t| t.duration_ms).sum::<u64>()
    );
}

#[test]
fn id_lists_are_not_limited_by_sqlite_variable_count() {
    let (library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    let ids: Vec<i64> = (1_000..51_000).chain([halo]).collect();

    let found = library
        .tracks(&TrackQuery {
            ids: Some(ids),
            ..all_kinds()
        })
        .unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, halo);
}

#[test]
fn json_contract_uses_camel_case_and_query_defaults() {
    let (library, _dir) = scanned_library();
    let halo = serde_json::to_value(track_titled(&library, "Halo")).unwrap();

    assert_eq!(halo["albumArtist"], "Beyoncé");
    assert_eq!(halo["kind"], "music");
    assert!(halo["durationMs"].is_u64());
    assert!(halo["sortTitle"].is_null());

    let query: TrackQuery =
        serde_json::from_str(r#"{"sort":"albumArtist","descending":true,"search":"x"}"#).unwrap();
    assert_eq!(query.kind, Some(MediaKind::Music));
    assert_eq!(query.sort, SortKey::AlbumArtist);
    assert!(query.descending);
    assert_eq!(query.ids, None);
    let all: TrackQuery = serde_json::from_str(r#"{"kind":null}"#).unwrap();
    assert_eq!(all.kind, None);
    let scoped: TrackQuery = serde_json::from_str(r#"{"ids":[3,5]}"#).unwrap();
    assert_eq!(scoped.ids, Some(vec![3, 5]));
}
