use std::fs;
use std::time::Instant;

use unsub_library::{ExternalTrack, Library, MatchQuality, MediaKind, Track, TrackMatch};

use crate::common::{library_of, scanned_library, track, track_titled};

fn external(title: &str, artist: &str, album: &str, duration_ms: Option<u64>) -> ExternalTrack {
    ExternalTrack {
        external_id: format!("apple:{title}"),
        title: title.into(),
        artist: artist.into(),
        album_artist: artist.into(),
        album: album.into(),
        duration_ms,
        track_number: None,
        disc_number: None,
        path: None,
    }
}

fn outcome(found: &Option<TrackMatch>) -> Option<(i64, MatchQuality)> {
    found.as_ref().map(|m| (m.track_id, m.quality))
}

fn match_one(library: &Library, external: ExternalTrack) -> Option<(i64, MatchQuality)> {
    let results = library.match_external(&[external]).unwrap();
    assert_eq!(results.len(), 1);
    outcome(&results[0])
}

const SONG_MS: u64 = 200_000;

#[test]
fn exact_needs_title_artist_and_duration_within_three_seconds() {
    let library = library_of(&[track(1, "Halo", "Beyoncé", "I Am", SONG_MS)]);
    let at = |ms| match_one(&library, external("Halo", "Beyoncé", "Other", Some(ms)));

    assert_eq!(at(SONG_MS), Some((1, MatchQuality::Exact)));
    assert_eq!(at(SONG_MS + 3_000), Some((1, MatchQuality::Exact)));
    assert_eq!(at(SONG_MS - 3_000), Some((1, MatchQuality::Exact)));
    assert_eq!(at(SONG_MS + 3_001), Some((1, MatchQuality::Close)));
    assert_eq!(at(SONG_MS - 8_000), Some((1, MatchQuality::Close)));
    assert_eq!(at(SONG_MS + 8_001), None);
}

#[test]
fn missing_duration_falls_back_to_the_album() {
    let library = library_of(&[track(1, "Halo", "Beyoncé", "I Am... Sasha Fierce", SONG_MS)]);
    let ask = |album, duration| match_one(&library, external("Halo", "Beyoncé", album, duration));

    assert_eq!(
        ask("I Am… Sasha Fierce", None),
        Some((1, MatchQuality::Exact))
    );
    assert_eq!(ask("Halo - Single", None), Some((1, MatchQuality::Close)));
    assert_eq!(ask("", None), Some((1, MatchQuality::Close)));
}

#[test]
fn an_unreadable_library_duration_counts_as_missing() {
    let library = library_of(&[track(1, "Halo", "Beyoncé", "I Am", 0)]);
    let ask = |album| match_one(&library, external("Halo", "Beyoncé", album, Some(SONG_MS)));

    assert_eq!(ask("I Am"), Some((1, MatchQuality::Exact)));
    assert_eq!(ask("Elsewhere"), Some((1, MatchQuality::Close)));
}

#[test]
fn an_empty_album_never_counts_as_the_same_album() {
    let library = library_of(&[track(1, "Halo", "Beyoncé", "", SONG_MS)]);

    assert_eq!(
        match_one(&library, external("Halo", "Beyoncé", "", None)),
        Some((1, MatchQuality::Close))
    );
}

#[test]
fn close_ignores_album_differences_such_as_single_versus_album() {
    let library = library_of(&[track(1, "Halo", "Beyoncé", "I Am... Sasha Fierce", SONG_MS)]);

    assert_eq!(
        match_one(
            &library,
            external("Halo", "Beyoncé", "Halo - Single", Some(SONG_MS + 5_000))
        ),
        Some((1, MatchQuality::Close))
    );
}

#[test]
fn different_songs_and_different_artists_do_not_match() {
    let library = library_of(&[track(
        1,
        "Hurt",
        "Nine Inch Nails",
        "The Downward Spiral",
        373_000,
    )]);
    let ask = |title, artist| match_one(&library, external(title, artist, "", Some(373_000)));

    assert_eq!(
        ask("Hurt", "Nine Inch Nails"),
        Some((1, MatchQuality::Exact))
    );
    assert_eq!(ask("Hurt", "Johnny Cash"), None);
    assert_eq!(ask("Closer", "Nine Inch Nails"), None);
    assert_eq!(ask("Hurts", "Nine Inch Nails"), None);
}

#[test]
fn live_remix_and_other_recordings_do_not_match_the_studio_version() {
    let library = library_of(&[track(1, "Song", "Band", "Album", SONG_MS)]);
    let variants = [
        "Song (Live)",
        "Song (Live at Wembley)",
        "Song - Live",
        "Song (Remix)",
        "Song (Dave Remix)",
        "Song (Acoustic)",
        "Song (Demo)",
        "Song (Instrumental)",
        "Song (Club Edit)",
    ];
    for variant in variants {
        assert_eq!(
            match_one(&library, external(variant, "Band", "Album", Some(SONG_MS))),
            None,
            "{variant}"
        );
    }

    let live = library_of(&[track(1, "Song (Live)", "Band", "Live Album", SONG_MS)]);
    assert_eq!(
        match_one(&live, external("Song", "Band", "Album", Some(SONG_MS))),
        None
    );
    assert_eq!(
        match_one(
            &live,
            external("Song - Live", "Band", "Live Album", Some(SONG_MS))
        ),
        Some((1, MatchQuality::Exact))
    );
}

#[test]
fn remasters_and_featured_artists_still_match() {
    let library = library_of(&[
        track(1, "Come Together", "The Beatles", "Abbey Road", SONG_MS),
        track(
            2,
            "Get Lucky",
            "Daft Punk",
            "Random Access Memories",
            SONG_MS,
        ),
    ]);

    let come_together = external(
        "Come Together - Remastered 2009",
        "The Beatles",
        "Abbey Road (Remastered)",
        Some(SONG_MS + 1_000),
    );
    assert_eq!(
        match_one(&library, come_together),
        Some((1, MatchQuality::Exact))
    );
    let get_lucky = external(
        "Get Lucky (feat. Pharrell Williams & Nile Rodgers)",
        "Daft Punk feat. Pharrell Williams",
        "Random Access Memories",
        Some(SONG_MS),
    );
    assert_eq!(
        match_one(&library, get_lucky),
        Some((2, MatchQuality::Exact))
    );
}

#[test]
fn artists_match_through_either_artist_field() {
    let mut compilation_track = track(1, "Song", "Soloist", "Tribute", SONG_MS);
    compilation_track.album_artist = "The Band".into();
    let library = library_of(&[compilation_track]);
    let ask = |artist: &str, album_artist: &str| {
        let mut ext = external("Song", artist, "Tribute", Some(SONG_MS));
        ext.album_artist = album_artist.into();
        match_one(&library, ext)
    };

    assert_eq!(ask("Soloist", "Soloist"), Some((1, MatchQuality::Exact)));
    assert_eq!(ask("Band", "Band"), Some((1, MatchQuality::Exact)));
    assert_eq!(
        ask("Someone Else", "The Band"),
        Some((1, MatchQuality::Exact))
    );
    assert_eq!(ask("Someone Else", "Another Band"), None);
}

#[test]
fn placeholder_and_missing_artists_never_identify_a_song() {
    let mut anonymous = track(1, "Song", "Various Artists", "Compilation", SONG_MS);
    anonymous.album_artist = "Various Artists".into();
    let blank = track(2, "Other", "", "", SONG_MS);
    let library = library_of(&[anonymous, blank]);

    assert_eq!(
        match_one(
            &library,
            external("Song", "Various Artists", "Compilation", Some(SONG_MS))
        ),
        None
    );
    assert_eq!(
        match_one(
            &library,
            external("Other", "Unknown Artist", "", Some(SONG_MS))
        ),
        None
    );
    assert_eq!(
        match_one(&library, external("Other", "", "", Some(SONG_MS))),
        None
    );
}

#[test]
fn a_real_artist_matches_even_when_the_album_artist_is_a_placeholder() {
    let mut compilation_track = track(1, "Song", "Real Artist", "Hits", SONG_MS);
    compilation_track.album_artist = "Various Artists".into();
    let library = library_of(&[compilation_track]);
    let mut ext = external("Song", "Real Artist", "Hits", Some(SONG_MS));
    ext.album_artist = "Various Artists".into();

    assert_eq!(match_one(&library, ext), Some((1, MatchQuality::Exact)));
}

#[test]
fn blank_titles_never_match() {
    let library = library_of(&[track(1, "", "Band", "Album", SONG_MS)]);

    assert_eq!(
        match_one(&library, external("", "Band", "Album", Some(SONG_MS))),
        None
    );
}

#[test]
fn exact_beats_close_even_when_the_close_one_shares_the_album() {
    let library = library_of(&[
        track(1, "Song", "Band", "Album", SONG_MS + 6_000),
        track(2, "Song", "Band", "Other", SONG_MS + 2_000),
    ]);

    assert_eq!(
        match_one(&library, external("Song", "Band", "Album", Some(SONG_MS))),
        Some((2, MatchQuality::Exact))
    );
}

#[test]
fn within_a_tier_same_album_wins_then_closest_duration_then_lowest_id() {
    let library = library_of(&[
        track(5, "Song", "Band", "Greatest Hits", SONG_MS + 1_500),
        track(6, "Song", "Band", "Greatest Hits", SONG_MS + 500),
        track(7, "Song", "Band", "Original Album", SONG_MS),
        track(8, "Song", "Band", "Original Album", SONG_MS),
    ]);
    let ask = |album| match_one(&library, external("Song", "Band", album, Some(SONG_MS)));

    // The same album outranks a perfect duration; the nearer duration breaks the tie within it.
    assert_eq!(ask("Greatest Hits"), Some((6, MatchQuality::Exact)));
    // Equal album and duration: the lowest id.
    assert_eq!(ask("Original Album"), Some((7, MatchQuality::Exact)));
    // No album in common: the closest duration, then the lowest id.
    assert_eq!(ask("Elsewhere"), Some((7, MatchQuality::Exact)));
}

#[test]
fn the_choice_does_not_depend_on_library_order() {
    let tracks = [
        track(3, "Song", "Band", "A", SONG_MS + 1_000),
        track(1, "Song", "Band", "B", SONG_MS + 1_000),
        track(2, "Song", "Band", "C", SONG_MS + 1_000),
    ];
    let reversed: Vec<Track> = tracks.iter().rev().cloned().collect();
    let ask = |library: &Library| match_one(library, external("Song", "Band", "Z", Some(SONG_MS)));

    assert_eq!(ask(&library_of(&tracks)), Some((1, MatchQuality::Exact)));
    assert_eq!(ask(&library_of(&reversed)), Some((1, MatchQuality::Exact)));
}

#[test]
fn a_track_matching_through_both_artist_fields_is_chosen_once() {
    let mut both = track(1, "Song", "Band", "Album", SONG_MS);
    both.album_artist = "The Band".into();
    let library = library_of(&[both]);

    assert_eq!(
        match_one(&library, external("Song", "Band", "Album", Some(SONG_MS))),
        Some((1, MatchQuality::Exact))
    );
}

#[test]
fn results_follow_input_order_and_duplicates_may_share_a_track() {
    let library = library_of(&[track(1, "Halo", "Beyoncé", "I Am", SONG_MS)]);
    let mut first = external("Halo", "Beyoncé", "I Am", Some(SONG_MS));
    first.external_id = "a".into();
    let mut miss = external("Nope", "Nobody", "", Some(SONG_MS));
    miss.external_id = "b".into();
    let mut duplicate = external("Halo", "Beyonce", "I Am", Some(SONG_MS + 500));
    duplicate.external_id = "c".into();
    let inputs = [first, miss, duplicate];

    let results = library.match_external(&inputs).unwrap();

    assert_eq!(results.len(), 3);
    assert_eq!(
        results[0].as_ref().map(|m| m.external_id.as_str()),
        Some("a")
    );
    assert!(results[1].is_none());
    assert_eq!(
        results[2].as_ref().map(|m| m.external_id.as_str()),
        Some("c")
    );
    assert_eq!(outcome(&results[0]), outcome(&results[2]));
    let again = library.match_external(&inputs).unwrap();
    assert_eq!(
        results.iter().map(outcome).collect::<Vec<_>>(),
        again.iter().map(outcome).collect::<Vec<_>>()
    );
}

#[test]
fn an_empty_library_or_input_matches_nothing() {
    let empty = Library::open_in_memory().unwrap();
    let inputs = [
        external("Halo", "Beyoncé", "", Some(SONG_MS)),
        external("Hurt", "NIN", "", None),
    ];

    assert!(
        empty
            .match_external(&inputs)
            .unwrap()
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(empty.match_external(&inputs).unwrap().len(), 2);
    assert!(empty.match_external(&[]).unwrap().is_empty());
    let populated = library_of(&[track(1, "Halo", "Beyoncé", "", SONG_MS)]);
    assert!(populated.match_external(&[]).unwrap().is_empty());
}

#[test]
fn every_media_kind_is_searched() {
    let mut episode = track(1, "Episode 12", "Radiolab", "Radiolab", SONG_MS);
    episode.kind = MediaKind::Podcast;
    let mut chapter = track(2, "Chapter 1", "Jane Austen", "Emma", SONG_MS);
    chapter.kind = MediaKind::Audiobook;
    let library = library_of(&[episode, chapter]);

    assert_eq!(
        match_one(
            &library,
            external("Episode 12", "Radiolab", "Radiolab", Some(SONG_MS))
        ),
        Some((1, MatchQuality::Exact))
    );
    assert_eq!(
        match_one(
            &library,
            external("Chapter 1", "Jane Austen", "Emma", Some(SONG_MS))
        ),
        Some((2, MatchQuality::Exact))
    );
}

#[test]
fn scanned_files_match_by_their_tags() {
    let (library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo");
    let ask = |ext| match_one(&library, ext);

    let by_tags = external(
        "Halo",
        "Beyonce",
        "I Am… Sasha Fierce",
        Some(halo.duration_ms),
    );
    assert_eq!(ask(by_tags), Some((halo.id, MatchQuality::Exact)));
    let other_artist = external("Halo", "Haim", "Days Are Gone", Some(halo.duration_ms));
    assert_eq!(ask(other_artist), None);
}

#[test]
fn same_file_is_recognised_by_canonical_path_and_beats_the_tags() {
    let (library, dir) = scanned_library();
    let halo = track_titled(&library, "Halo");
    let zombie = track_titled(&library, "Zombie");

    // The temp dir may sit behind a symlink (/var on macOS), so this path is
    // not the stored canonical one, and the title says nothing about the file.
    let mut by_path = external("Retagged Elsewhere", "Nobody", "", None);
    by_path.path = Some(dir.path().join("halo.mp3").to_str().unwrap().into());
    assert_eq!(
        match_one(&library, by_path),
        Some((halo.id, MatchQuality::SameFile))
    );

    let mut contradicting = external("Halo", "Beyoncé", "", Some(halo.duration_ms));
    contradicting.path = Some(zombie.path.clone());
    assert_eq!(
        match_one(&library, contradicting),
        Some((zombie.id, MatchQuality::SameFile))
    );
}

#[cfg(unix)]
#[test]
fn same_file_resolves_symlinked_and_dotted_paths() {
    let (library, dir) = scanned_library();
    let halo = track_titled(&library, "Halo");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(dir.path(), &link).unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();

    for path in [
        link.join("halo.mp3"),
        dir.path().join("./halo.mp3"),
        dir.path().join("sub/../halo.mp3"),
    ] {
        let mut ext = external("x", "y", "", None);
        ext.path = Some(path.to_str().unwrap().into());
        assert_eq!(
            match_one(&library, ext),
            Some((halo.id, MatchQuality::SameFile)),
            "{path:?}"
        );
    }
}

#[test]
fn a_path_that_resolves_to_nothing_falls_back_to_the_tags() {
    let (library, dir) = scanned_library();
    let halo = track_titled(&library, "Halo");
    let other_file = dir.path().join("not_in_library.mp3");
    fs::write(&other_file, "x").unwrap();

    for path in ["/definitely/not/here.mp3", other_file.to_str().unwrap(), ""] {
        let mut by_tags = external("Halo", "Beyoncé", "", Some(halo.duration_ms));
        by_tags.path = Some(path.into());
        assert_eq!(
            match_one(&library, by_tags),
            Some((halo.id, MatchQuality::Exact)),
            "{path:?}"
        );

        let mut unrelated = external("Nope", "Nobody", "", None);
        unrelated.path = Some(path.into());
        assert_eq!(match_one(&library, unrelated), None, "{path:?}");
    }
}

#[test]
fn json_contract_is_camel_case() {
    let ext: ExternalTrack = serde_json::from_str(
        r#"{"externalId":"x1","title":"Halo","artist":"Beyoncé","albumArtist":"Beyoncé",
            "album":"I Am","durationMs":261000,"trackNumber":3,"discNumber":null,"path":null}"#,
    )
    .unwrap();
    assert_eq!(
        (ext.external_id.as_str(), ext.duration_ms, ext.track_number),
        ("x1", Some(261_000), Some(3))
    );
    assert_eq!(ext.path, None);

    let found = TrackMatch {
        external_id: "x1".into(),
        track_id: 7,
        quality: MatchQuality::SameFile,
    };
    assert_eq!(
        serde_json::to_value(found).unwrap(),
        serde_json::json!({"externalId": "x1", "trackId": 7, "quality": "sameFile"})
    );
}

/// Timing sanity check, kept out of the default run because debug builds are
/// an order of magnitude slower. Run with:
/// `cargo test -p unsub-library --release -- --ignored matching_scales`
#[test]
#[ignore = "timing check; needs --release"]
fn matching_scales_to_fifty_thousand_against_a_hundred_thousand() {
    let tracks: Vec<Track> = (0..100_000_i64)
        .map(|i| {
            track(
                i + 1,
                &format!("Song Number {i} (Remastered 2009)"),
                &format!("Artist {}", i % 2_000),
                &format!("Album {}", i % 8_000),
                180_000 + (i as u64 % 60) * 1_000,
            )
        })
        .collect();
    let library = library_of(&tracks);
    let inputs: Vec<ExternalTrack> = (0..50_000_i64)
        .map(|i| {
            // Even entries exist in the library; odd ones do not.
            let source = if i % 2 == 0 { i * 2 } else { 1_000_000 + i };
            let mut ext = external(
                &format!("Song Number {source}"),
                &format!("Artist {} feat. Guest", source % 2_000),
                &format!("Album {}", source % 8_000),
                Some(180_000 + (source as u64 % 60) * 1_000 + 700),
            );
            ext.external_id = i.to_string();
            ext.path = (i % 10 == 0).then(|| format!("/Volumes/Music/{i}.m4a"));
            ext
        })
        .collect();

    let started = Instant::now();
    let results = library.match_external(&inputs).unwrap();
    let elapsed = started.elapsed();

    println!("matched 50k against 100k in {elapsed:?}");
    assert_eq!(results.iter().flatten().count(), 25_000);
    assert!(elapsed.as_secs_f64() < 1.0, "took {elapsed:?}");
}
