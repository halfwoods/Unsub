use antistreaming_library::{
    DateUnit, Error, Library, LimitOrder, LimitUnit, PlaylistKind, SmartField, SmartLimit, SmartOp,
    SmartRule, SmartRules, SmartValue, SortKey, Track, TrackQuery,
};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::common::{library_of, track};

const DAY: i64 = 86_400;
const JAN_10_2020: i64 = 1_578_614_400;
const JAN_11_2020: i64 = JAN_10_2020 + DAY;
const JUN_1_2021: i64 = 1_622_505_600;
const MAR_1_2022: i64 = 1_646_092_800;
const JAN_1_2023: i64 = 1_672_531_200;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Six tracks whose fields differ enough to tell every rule apart. Name order
/// is Alpha, Beta, delta, Epsilon, Gamma Ray, Zéta.
fn dataset() -> Library {
    let now = now();
    let tracks = [
        Track {
            album_artist: String::new(),
            genre: "Rock".into(),
            composer: "Bach".into(),
            year: Some(1991),
            track_number: Some(1),
            disc_number: Some(1),
            rating: 100,
            play_count: 10,
            file_size: 4_000_000,
            bitrate_kbps: Some(256),
            sample_rate: Some(44_100),
            date_added: JAN_10_2020 + 12 * 3_600,
            last_played: Some(now - DAY),
            ..track(1, "Alpha", "Ann", "First", 180_000)
        },
        Track {
            album_artist: String::new(),
            genre: "Pop".into(),
            year: Some(1995),
            track_number: Some(2),
            disc_number: Some(1),
            rating: 80,
            play_count: 5,
            skip_count: 1,
            file_size: 6_000_000,
            bitrate_kbps: Some(320),
            sample_rate: Some(44_100),
            codec: "AAC".into(),
            date_added: JAN_10_2020 + 23 * 3_600,
            last_played: Some(now - 10 * DAY),
            ..track(2, "Beta", "Ann", "First", 240_000)
        },
        Track {
            album_artist: String::new(),
            genre: "Rock".into(),
            composer: "Chopin".into(),
            year: Some(2001),
            track_number: Some(3),
            disc_number: Some(2),
            rating: 60,
            file_size: 30_000_000,
            bitrate_kbps: Some(900),
            sample_rate: Some(96_000),
            codec: "FLAC".into(),
            enabled: false,
            date_added: JAN_11_2020 + 3_600,
            last_played: Some(now - 20 * DAY),
            ..track(3, "Gamma Ray", "Bob", "Second", 300_000)
        },
        Track {
            album_artist: "Bob & Co".into(),
            genre: "Classical".into(),
            composer: "Bach".into(),
            year: Some(2010),
            track_number: Some(4),
            disc_number: Some(2),
            rating: 40,
            play_count: 3,
            file_size: 2_000_000,
            bitrate_kbps: Some(128),
            sample_rate: Some(22_050),
            date_added: JUN_1_2021,
            last_played: Some(now - 100 * DAY),
            ..track(4, "delta", "Bob", "Second", 120_000)
        },
        Track {
            album_artist: String::new(),
            rating: 20,
            play_count: 1,
            file_size: 1_000_000,
            compilation: true,
            date_added: MAR_1_2022,
            ..track(5, "Epsilon", "Cat", "Hits", 60_000)
        },
        Track {
            album_artist: String::new(),
            genre: "pop".into(),
            year: Some(1999),
            file_size: 1_500_000_000,
            bitrate_kbps: Some(192),
            sample_rate: Some(48_000),
            codec: "WAV".into(),
            date_added: JAN_1_2023,
            ..track(6, "Z\u{e9}ta", "Dan", "", 3_600_000)
        },
    ];
    let mut library = library_of(&tracks);
    library.record_skip(2, now - 3 * DAY).unwrap();
    library.record_skip(4, now - 100 * DAY).unwrap();
    library
}

fn smart_playlist(library: &mut Library) -> i64 {
    library
        .create_playlist("Smart", PlaylistKind::Smart, None)
        .unwrap()
        .id
}

fn listed(library: &Library, playlist: i64) -> Vec<String> {
    library
        .tracks(&TrackQuery {
            kind: None,
            playlist: Some(playlist),
            sort: SortKey::Name,
            ..TrackQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|track| track.title)
        .collect()
}

fn evaluate(library: &mut Library, playlist: i64, rules: &SmartRules) -> Vec<String> {
    library.set_smart_rules(playlist, rules).unwrap();
    listed(library, playlist)
}

fn cond(field: SmartField, op: SmartOp, value: SmartValue) -> SmartRule {
    SmartRule::Condition { field, op, value }
}

fn all(rules: Vec<SmartRule>) -> SmartRules {
    SmartRules {
        rules,
        ..SmartRules::default()
    }
}

fn any(rules: Vec<SmartRule>) -> SmartRules {
    SmartRules {
        match_all: false,
        rules,
        ..SmartRules::default()
    }
}

fn text(value: &str) -> SmartValue {
    SmartValue::Text(value.into())
}

fn range(from: i64, to: i64) -> SmartValue {
    SmartValue::Range { from, to }
}

fn within(amount: u32, unit: DateUnit) -> SmartValue {
    SmartValue::Relative { amount, unit }
}

type Case = (SmartField, SmartOp, SmartValue, &'static [&'static str]);

fn check(cases: Vec<Case>) {
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    for (field, op, value, expected) in cases {
        let rules = all(vec![cond(field, op, value.clone())]);
        assert_eq!(
            evaluate(&mut library, playlist, &rules),
            expected,
            "{field:?} {op:?} {value:?}"
        );
    }
}

#[test]
fn text_conditions_fold_case_and_diacritics() {
    use SmartField::*;
    use SmartOp::*;
    check(vec![
        (
            Name,
            Contains,
            text("A"),
            &["Alpha", "Beta", "delta", "Gamma Ray", "Z\u{e9}ta"],
        ),
        (Name, DoesNotContain, text("a"), &["Epsilon"]),
        (Name, Is, text("ZETA"), &["Z\u{e9}ta"]),
        (
            Name,
            IsNot,
            text("zeta"),
            &["Alpha", "Beta", "delta", "Epsilon", "Gamma Ray"],
        ),
        (Name, StartsWith, text("GAM"), &["Gamma Ray"]),
        (Name, EndsWith, text("Ray"), &["Gamma Ray"]),
        (Name, EndsWith, text("ta"), &["Beta", "delta", "Z\u{e9}ta"]),
        (
            Name,
            StartsWith,
            text(""),
            &[
                "Alpha",
                "Beta",
                "delta",
                "Epsilon",
                "Gamma Ray",
                "Z\u{e9}ta",
            ],
        ),
        (
            Name,
            EndsWith,
            text(""),
            &[
                "Alpha",
                "Beta",
                "delta",
                "Epsilon",
                "Gamma Ray",
                "Z\u{e9}ta",
            ],
        ),
        (Name, StartsWith, text("gamma rayx"), &[]),
        (Name, EndsWith, text("xgamma ray"), &[]),
        (
            Artist,
            Contains,
            text("an"),
            &["Alpha", "Beta", "Z\u{e9}ta"],
        ),
        (Artist, Is, text("bob"), &["delta", "Gamma Ray"]),
        (
            AlbumArtist,
            Is,
            text(""),
            &["Alpha", "Beta", "Epsilon", "Gamma Ray", "Z\u{e9}ta"],
        ),
        (AlbumArtist, Is, text("BOB & co"), &["delta"]),
        (Album, StartsWith, text("sec"), &["delta", "Gamma Ray"]),
        (Album, Is, text(""), &["Z\u{e9}ta"]),
        (Genre, Is, text("ROCK"), &["Alpha", "Gamma Ray"]),
        (Genre, Is, text("pop"), &["Beta", "Z\u{e9}ta"]),
        (Genre, Contains, text("ass"), &["delta"]),
        (Composer, Contains, text("ba"), &["Alpha", "delta"]),
        (Kind, Is, text("mp3"), &["Alpha", "delta", "Epsilon"]),
        (
            Kind,
            IsNot,
            text("MP3"),
            &["Beta", "Gamma Ray", "Z\u{e9}ta"],
        ),
    ]);
}

#[test]
fn text_conditions_treat_sql_wildcards_literally() {
    use SmartField::*;
    use SmartOp::*;
    check(vec![
        (Name, Contains, text("%"), &[]),
        (Name, Contains, text("_"), &[]),
        (Name, StartsWith, text("a%"), &[]),
        (Name, Is, text("' OR 1=1 --"), &[]),
    ]);
}

#[test]
fn number_conditions_compare_stored_units() {
    use SmartField::*;
    use SmartOp::*;
    let n = SmartValue::Number;
    check(vec![
        (Year, Is, n(1995), &["Beta"]),
        (
            Year,
            IsNot,
            n(1995),
            &["Alpha", "delta", "Epsilon", "Gamma Ray", "Z\u{e9}ta"],
        ),
        (Year, GreaterThan, n(1999), &["delta", "Gamma Ray"]),
        (Year, LessThan, n(1995), &["Alpha"]),
        (Year, InRange, range(1995, 1999), &["Beta", "Z\u{e9}ta"]),
        (Year, InRange, range(1999, 1995), &["Beta", "Z\u{e9}ta"]),
        (Rating, GreaterThan, n(80), &["Alpha"]),
        (Rating, Is, n(0), &["Z\u{e9}ta"]),
        (Rating, LessThan, n(40), &["Epsilon", "Z\u{e9}ta"]),
        (
            Rating,
            InRange,
            range(40, 80),
            &["Beta", "delta", "Gamma Ray"],
        ),
        (Plays, Is, n(0), &["Gamma Ray", "Z\u{e9}ta"]),
        (Plays, GreaterThan, n(4), &["Alpha", "Beta"]),
        (Skips, GreaterThan, n(0), &["Beta", "delta"]),
        (
            Time,
            GreaterThan,
            n(200_000),
            &["Beta", "Gamma Ray", "Z\u{e9}ta"],
        ),
        (Time, LessThan, n(100_000), &["Epsilon"]),
        (Time, InRange, range(120_000, 180_000), &["Alpha", "delta"]),
        (BitRate, GreaterThan, n(256), &["Beta", "Gamma Ray"]),
        (
            BitRate,
            IsNot,
            n(256),
            &["Beta", "delta", "Epsilon", "Gamma Ray", "Z\u{e9}ta"],
        ),
        (SampleRate, Is, n(44_100), &["Alpha", "Beta"]),
        (
            Size,
            GreaterThan,
            n(10_000_000),
            &["Gamma Ray", "Z\u{e9}ta"],
        ),
        (TrackNumber, Is, n(3), &["Gamma Ray"]),
        (DiscNumber, Is, n(2), &["delta", "Gamma Ray"]),
    ]);
}

#[test]
fn absolute_date_conditions_work_on_whole_utc_days() {
    use SmartField::DateAdded;
    use SmartOp::*;
    let n = SmartValue::Number;
    check(vec![
        (DateAdded, Is, n(JAN_10_2020), &["Alpha", "Beta"]),
        (
            DateAdded,
            Is,
            n(JAN_10_2020 + 5 * 3_600),
            &["Alpha", "Beta"],
        ),
        (DateAdded, Is, n(JAN_11_2020), &["Gamma Ray"]),
        (DateAdded, Before, n(JAN_11_2020), &["Alpha", "Beta"]),
        (
            DateAdded,
            After,
            n(JAN_11_2020),
            &["delta", "Epsilon", "Z\u{e9}ta"],
        ),
        (
            DateAdded,
            InRange,
            range(JAN_10_2020 + 12 * 3_600, JAN_11_2020 + 3_600),
            &["Alpha", "Beta", "Gamma Ray"],
        ),
    ]);
}

#[test]
fn relative_date_conditions_count_back_from_now() {
    use DateUnit::*;
    use SmartField::{LastPlayed, LastSkipped};
    use SmartOp::*;
    check(vec![
        (LastPlayed, InTheLast, within(2, Days), &["Alpha"]),
        (LastPlayed, InTheLast, within(2, Weeks), &["Alpha", "Beta"]),
        (
            LastPlayed,
            InTheLast,
            within(1, Months),
            &["Alpha", "Beta", "Gamma Ray"],
        ),
        (
            LastPlayed,
            InTheLast,
            within(3, Months),
            &["Alpha", "Beta", "Gamma Ray"],
        ),
        (
            LastPlayed,
            InTheLast,
            within(4, Months),
            &["Alpha", "Beta", "delta", "Gamma Ray"],
        ),
        // A track that was never played is not in the last two weeks either.
        (
            LastPlayed,
            NotInTheLast,
            within(2, Weeks),
            &["delta", "Epsilon", "Gamma Ray", "Z\u{e9}ta"],
        ),
        (LastSkipped, InTheLast, within(1, Weeks), &["Beta"]),
        (
            LastSkipped,
            NotInTheLast,
            within(1, Weeks),
            &["Alpha", "delta", "Epsilon", "Gamma Ray", "Z\u{e9}ta"],
        ),
    ]);
}

#[test]
fn flag_conditions_ignore_their_value() {
    use SmartField::*;
    use SmartOp::*;
    check(vec![
        (
            Checked,
            IsTrue,
            SmartValue::Empty,
            &["Alpha", "Beta", "delta", "Epsilon", "Z\u{e9}ta"],
        ),
        (Checked, IsFalse, SmartValue::Empty, &["Gamma Ray"]),
        (Compilation, IsTrue, text("whatever"), &["Epsilon"]),
        (
            Compilation,
            IsFalse,
            SmartValue::Empty,
            &["Alpha", "Beta", "delta", "Gamma Ray", "Z\u{e9}ta"],
        ),
    ]);
}

#[test]
fn conditions_combine_with_all_any_and_nested_groups() {
    use SmartField::*;
    use SmartOp::*;
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    let rock = || cond(Genre, Is, text("rock"));
    let big_rating = || cond(Rating, GreaterThan, SmartValue::Number(90));
    let recent_year = || cond(Year, GreaterThan, SmartValue::Number(2000));

    assert_eq!(
        evaluate(&mut library, playlist, &all(vec![rock(), big_rating()])),
        ["Alpha"]
    );
    assert_eq!(
        evaluate(
            &mut library,
            playlist,
            &any(vec![rock(), cond(Year, Is, SmartValue::Number(1995))])
        ),
        ["Alpha", "Beta", "Gamma Ray"]
    );
    // rock AND (rating > 90 OR year > 2000)
    let nested = all(vec![
        rock(),
        SmartRule::Group(any(vec![big_rating(), recent_year()])),
    ]);
    assert_eq!(
        evaluate(&mut library, playlist, &nested),
        ["Alpha", "Gamma Ray"]
    );
    // (rock AND year > 2000) OR rating is 0
    let either = any(vec![
        SmartRule::Group(all(vec![rock(), recent_year()])),
        cond(Rating, Is, SmartValue::Number(0)),
    ]);
    assert_eq!(
        evaluate(&mut library, playlist, &either),
        ["Gamma Ray", "Z\u{e9}ta"]
    );
    // Groups nest as deep as the editor allows.
    let deep = all(vec![SmartRule::Group(all(vec![SmartRule::Group(any(
        vec![rock()],
    ))]))]);
    assert_eq!(
        evaluate(&mut library, playlist, &deep),
        ["Alpha", "Gamma Ray"]
    );
}

#[test]
fn no_rules_match_everything_whichever_way_they_combine() {
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    let everything = [
        "Alpha",
        "Beta",
        "delta",
        "Epsilon",
        "Gamma Ray",
        "Z\u{e9}ta",
    ];

    assert_eq!(evaluate(&mut library, playlist, &all(vec![])), everything);
    assert_eq!(evaluate(&mut library, playlist, &any(vec![])), everything);
    let empty_group = all(vec![SmartRule::Group(all(vec![]))]);
    assert_eq!(evaluate(&mut library, playlist, &empty_group), everything);
}

#[test]
fn only_checked_drops_unchecked_songs() {
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    let rules = SmartRules {
        only_checked: true,
        ..SmartRules::default()
    };

    assert_eq!(
        evaluate(&mut library, playlist, &rules),
        ["Alpha", "Beta", "delta", "Epsilon", "Z\u{e9}ta"]
    );
}

#[test]
fn operators_that_do_not_suit_a_field_are_rejected_and_nothing_is_saved() {
    use SmartField::*;
    use SmartOp::*;
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    let good = all(vec![cond(Genre, Is, text("rock"))]);
    evaluate(&mut library, playlist, &good);

    let bad = [
        (Name, GreaterThan, SmartValue::Number(1)),
        (Name, Contains, SmartValue::Number(1)),
        (Name, InRange, range(1, 2)),
        (Year, Contains, text("1")),
        (Year, Is, text("1995")),
        (Year, InRange, SmartValue::Number(1)),
        (Rating, Before, SmartValue::Number(1)),
        (DateAdded, IsNot, SmartValue::Number(1)),
        (DateAdded, InTheLast, SmartValue::Number(1)),
        (LastPlayed, GreaterThan, SmartValue::Number(1)),
        (Checked, Contains, text("x")),
        (Checked, Is, SmartValue::Empty),
        (Playlist, Contains, SmartValue::Playlist(1)),
        (Playlist, Is, SmartValue::Number(1)),
    ];
    for (field, op, value) in bad {
        let result = library.set_smart_rules(playlist, &all(vec![cond(field, op, value.clone())]));
        assert!(
            matches!(result, Err(Error::InvalidRule(_))),
            "{field:?} {op:?} {value:?}: {result:?}"
        );
    }
    assert_eq!(
        listed(&library, playlist),
        ["Alpha", "Gamma Ray"],
        "the old rules survive"
    );
    let stored = library.playlists().unwrap();
    let stored = stored.iter().find(|p| p.id == playlist).unwrap();
    assert_eq!(stored.smart.as_ref(), Some(&good));
}

fn limited(amount: u32, unit: LimitUnit, selected_by: LimitOrder) -> SmartRules {
    SmartRules {
        limit: Some(SmartLimit {
            amount,
            unit,
            selected_by,
        }),
        ..SmartRules::default()
    }
}

#[test]
fn every_limit_order_picks_from_the_front_of_its_ordering() {
    use LimitOrder::*;
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    let cases: [(LimitOrder, [&str; 3]); 12] = [
        (Album, ["Alpha", "Beta", "Epsilon"]),
        (Artist, ["Alpha", "Beta", "Gamma Ray"]),
        (Genre, ["Beta", "delta", "Z\u{e9}ta"]),
        (Name, ["Alpha", "Beta", "delta"]),
        (HighestRating, ["Alpha", "Beta", "Gamma Ray"]),
        (LowestRating, ["delta", "Epsilon", "Z\u{e9}ta"]),
        (MostRecentlyPlayed, ["Alpha", "Beta", "Gamma Ray"]),
        (LeastRecentlyPlayed, ["delta", "Epsilon", "Z\u{e9}ta"]),
        (MostOftenPlayed, ["Alpha", "Beta", "delta"]),
        (LeastOftenPlayed, ["Epsilon", "Gamma Ray", "Z\u{e9}ta"]),
        (MostRecentlyAdded, ["delta", "Epsilon", "Z\u{e9}ta"]),
        (LeastRecentlyAdded, ["Alpha", "Beta", "Gamma Ray"]),
    ];
    for (order, expected) in cases {
        let rules = limited(3, LimitUnit::Items, order);
        assert_eq!(
            evaluate(&mut library, playlist, &rules),
            expected,
            "{order:?}"
        );
    }
}

#[test]
fn time_and_size_limits_stop_before_the_first_track_that_overshoots() {
    use LimitUnit::*;
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    // In name order the tracks run 3, 4, 2, 1, 5 and 60 minutes, and 4, 6, 2,
    // 1, 30 and 1500 MB.
    let cases: [(u32, LimitUnit, &[&str]); 9] = [
        (10, Minutes, &["Alpha", "Beta", "delta", "Epsilon"]),
        (9, Minutes, &["Alpha", "Beta", "delta"]),
        (2, Minutes, &[]),
        (
            1,
            Hours,
            &["Alpha", "Beta", "delta", "Epsilon", "Gamma Ray"],
        ),
        (
            2,
            Hours,
            &[
                "Alpha",
                "Beta",
                "delta",
                "Epsilon",
                "Gamma Ray",
                "Z\u{e9}ta",
            ],
        ),
        (13, Megabytes, &["Alpha", "Beta", "delta", "Epsilon"]),
        (12, Megabytes, &["Alpha", "Beta", "delta"]),
        (
            1,
            Gigabytes,
            &["Alpha", "Beta", "delta", "Epsilon", "Gamma Ray"],
        ),
        (
            2,
            Gigabytes,
            &[
                "Alpha",
                "Beta",
                "delta",
                "Epsilon",
                "Gamma Ray",
                "Z\u{e9}ta",
            ],
        ),
    ];
    for (amount, unit, expected) in cases {
        let rules = limited(amount, unit, LimitOrder::Name);
        assert_eq!(
            evaluate(&mut library, playlist, &rules),
            expected,
            "{amount} {unit:?}"
        );
    }
    let none = limited(0, Items, LimitOrder::Name);
    assert!(evaluate(&mut library, playlist, &none).is_empty());
}

#[test]
fn a_limit_applies_after_the_rules_and_the_checkbox() {
    use SmartField::*;
    use SmartOp::*;
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);

    let played = SmartRules {
        rules: vec![cond(Plays, GreaterThan, SmartValue::Number(0))],
        ..limited(2, LimitUnit::Items, LimitOrder::MostOftenPlayed)
    };
    assert_eq!(evaluate(&mut library, playlist, &played), ["Alpha", "Beta"]);

    let checked = SmartRules {
        only_checked: true,
        ..limited(3, LimitUnit::Items, LimitOrder::HighestRating)
    };
    assert_eq!(
        evaluate(&mut library, playlist, &checked),
        ["Alpha", "Beta", "delta"]
    );
}

#[test]
fn a_limited_playlist_still_takes_the_querys_search_sort_and_filters() {
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    library
        .set_smart_rules(
            playlist,
            &limited(4, LimitUnit::Items, LimitOrder::HighestRating),
        )
        .unwrap();
    let query = |search: Option<&str>, descending| TrackQuery {
        kind: None,
        playlist: Some(playlist),
        search: search.map(Into::into),
        sort: SortKey::Rating,
        descending,
        ..TrackQuery::default()
    };
    let titles = |query: &TrackQuery| -> Vec<String> {
        library
            .tracks(query)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    };

    // The four best-rated are Alpha, Beta, Gamma Ray and delta.
    assert_eq!(
        titles(&query(None, true)),
        ["Alpha", "Beta", "Gamma Ray", "delta"]
    );
    assert_eq!(
        titles(&query(None, false)),
        ["delta", "Gamma Ray", "Beta", "Alpha"]
    );
    assert_eq!(titles(&query(Some("bob"), false)), ["delta", "Gamma Ray"]);
    assert_eq!(library.stats(&query(Some("bob"), false)).unwrap().count, 2);
}

#[test]
fn random_limits_hold_still_until_the_rules_are_saved_again() {
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    let rules = limited(3, LimitUnit::Items, LimitOrder::Random);

    let first = evaluate(&mut library, playlist, &rules);
    assert_eq!(first.len(), 3);
    for _ in 0..3 {
        assert_eq!(listed(&library, playlist), first, "stable between calls");
    }
    library.rename_playlist(playlist, "Renamed").unwrap();
    assert_eq!(
        listed(&library, playlist),
        first,
        "renaming is not a modification"
    );

    let mut picks = vec![first];
    for _ in 0..12 {
        picks.push(evaluate(&mut library, playlist, &rules));
    }
    picks.sort();
    picks.dedup();
    assert!(picks.len() > 1, "saving re-rolls the pick: {picks:?}");
}

#[test]
fn a_frozen_playlist_keeps_its_snapshot_until_the_rules_are_saved_again() {
    use SmartField::Year;
    use SmartOp::LessThan;
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    let frozen = SmartRules {
        rules: vec![cond(Year, LessThan, SmartValue::Number(2000))],
        live: false,
        ..SmartRules::default()
    };
    assert_eq!(
        evaluate(&mut library, playlist, &frozen),
        ["Alpha", "Beta", "Z\u{e9}ta"]
    );

    library
        .import_tracks(&[Track {
            year: Some(1980),
            ..track(7, "Eta", "Eve", "Late", 100_000)
        }])
        .unwrap();
    assert_eq!(
        listed(&library, playlist),
        ["Alpha", "Beta", "Z\u{e9}ta"],
        "frozen"
    );
    assert_eq!(library.playlists().unwrap().last().unwrap().track_count, 3);

    assert_eq!(
        evaluate(&mut library, playlist, &frozen),
        ["Alpha", "Beta", "Eta", "Z\u{e9}ta"],
        "saving again re-takes the snapshot"
    );
    library.remove_tracks(&[7]).unwrap();
    assert_eq!(
        listed(&library, playlist),
        ["Alpha", "Beta", "Z\u{e9}ta"],
        "deleted tracks leave it"
    );

    let live = all(vec![cond(Year, LessThan, SmartValue::Number(1996))]);
    assert_eq!(evaluate(&mut library, playlist, &live), ["Alpha", "Beta"]);
}

#[test]
fn a_frozen_limited_playlist_lists_in_selection_order() {
    let mut library = dataset();
    let playlist = smart_playlist(&mut library);
    let rules = SmartRules {
        live: false,
        ..limited(3, LimitUnit::Items, LimitOrder::HighestRating)
    };
    library.set_smart_rules(playlist, &rules).unwrap();

    let in_order: Vec<String> = library
        .tracks(&TrackQuery {
            kind: None,
            playlist: Some(playlist),
            sort: SortKey::PlaylistOrder,
            ..TrackQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect();

    assert_eq!(in_order, ["Alpha", "Beta", "Gamma Ray"]);
}

#[test]
fn playlist_membership_rules_follow_regular_smart_and_folder_playlists() {
    use SmartField::Playlist;
    use SmartOp::*;
    let mut library = dataset();
    let regular = library
        .create_playlist("Mix", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library.add_to_playlist(regular, &[1, 5, 1], None).unwrap();
    let folder = library
        .create_playlist("Folder", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    let inner = library
        .create_playlist("Inner", PlaylistKind::Regular, Some(folder))
        .unwrap()
        .id;
    library.add_to_playlist(inner, &[2, 3], None).unwrap();
    let rocks = smart_playlist(&mut library);
    library
        .set_smart_rules(rocks, &all(vec![cond(SmartField::Genre, Is, text("rock"))]))
        .unwrap();
    let probe = library
        .create_playlist("Probe", PlaylistKind::Smart, None)
        .unwrap()
        .id;
    let member_of = |id| all(vec![cond(Playlist, Is, SmartValue::Playlist(id))]);

    assert_eq!(
        evaluate(&mut library, probe, &member_of(regular)),
        ["Alpha", "Epsilon"]
    );
    assert_eq!(
        evaluate(&mut library, probe, &member_of(folder)),
        ["Beta", "Gamma Ray"]
    );
    assert_eq!(
        evaluate(&mut library, probe, &member_of(rocks)),
        ["Alpha", "Gamma Ray"]
    );
    assert_eq!(
        evaluate(
            &mut library,
            probe,
            &all(vec![cond(Playlist, IsNot, SmartValue::Playlist(regular))])
        ),
        ["Beta", "delta", "Gamma Ray", "Z\u{e9}ta"]
    );
    let both = all(vec![
        cond(Playlist, Is, SmartValue::Playlist(regular)),
        cond(Playlist, IsNot, SmartValue::Playlist(rocks)),
    ]);
    assert_eq!(evaluate(&mut library, probe, &both), ["Epsilon"]);
}

#[test]
fn rules_that_refer_to_a_deleted_playlist_match_nothing_instead_of_breaking_listing() {
    use SmartField::Playlist;
    use SmartOp::*;
    let mut library = dataset();
    let doomed = library
        .create_playlist("Doomed", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library.add_to_playlist(doomed, &[1], None).unwrap();
    let is = smart_playlist(&mut library);
    let is_not = library
        .create_playlist("Is not", PlaylistKind::Smart, None)
        .unwrap()
        .id;
    library
        .set_smart_rules(
            is,
            &all(vec![cond(Playlist, Is, SmartValue::Playlist(doomed))]),
        )
        .unwrap();
    library
        .set_smart_rules(
            is_not,
            &all(vec![cond(Playlist, IsNot, SmartValue::Playlist(doomed))]),
        )
        .unwrap();

    library.delete_playlist(doomed).unwrap();

    assert!(listed(&library, is).is_empty());
    assert_eq!(listed(&library, is_not).len(), 6);
    library.playlists().unwrap();
    let saving = library.set_smart_rules(
        is,
        &all(vec![cond(Playlist, Is, SmartValue::Playlist(doomed))]),
    );
    assert!(matches!(saving, Err(Error::PlaylistNotFound(id)) if id == doomed));
}

#[test]
fn rules_that_would_refer_back_to_themselves_are_rejected() {
    use SmartField::Playlist;
    use SmartOp::Is;
    let mut library = dataset();
    let a = smart_playlist(&mut library);
    let b = library
        .create_playlist("B", PlaylistKind::Smart, None)
        .unwrap()
        .id;
    let folder = library
        .create_playlist("F", PlaylistKind::Folder, None)
        .unwrap()
        .id;
    let inside = library
        .create_playlist("S", PlaylistKind::Smart, Some(folder))
        .unwrap()
        .id;
    let refers_to = |id| all(vec![cond(Playlist, Is, SmartValue::Playlist(id))]);

    let itself = library.set_smart_rules(a, &refers_to(a));
    assert!(matches!(itself, Err(Error::PlaylistCycle)), "{itself:?}");
    library.set_smart_rules(a, &refers_to(b)).unwrap();
    let loop_back = library.set_smart_rules(b, &refers_to(a));
    assert!(
        matches!(loop_back, Err(Error::PlaylistCycle)),
        "{loop_back:?}"
    );
    let through_folder = library.set_smart_rules(inside, &refers_to(folder));
    assert!(
        matches!(through_folder, Err(Error::PlaylistCycle)),
        "{through_folder:?}"
    );
}
