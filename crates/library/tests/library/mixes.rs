use std::collections::HashSet;
use std::time::Instant;

use unsub_library::{Library, MediaKind, Mix, PlaylistKind, Track};

use crate::common::{library_of, track};

const DAY: i64 = 86_400;
const T0: i64 = 1_700_000_000;

fn song(id: i64, artist: &str, album: &str, genre: &str, year: i32, rating: u8) -> Track {
    Track {
        album_artist: String::new(),
        genre: genre.into(),
        year: Some(year),
        rating,
        ..track(id, &format!("song {id}"), artist, album, 200_000)
    }
}

/// Four big genres, a genre too small to mix, one artist big enough for a mix of
/// their own, and decoys that must never be offered.
fn catalogue() -> Vec<Track> {
    let mut tracks = Vec::new();
    for artist in 0..12 {
        for n in 0..3 {
            tracks.push(song(
                100 + artist * 3 + n,
                &format!("Rock Artist {artist}"),
                &format!("Rock Album {artist}"),
                "Rock",
                1990 + (artist % 6) as i32,
                (n * 20) as u8,
            ));
        }
    }
    for artist in 0..10 {
        for n in 0..3 {
            tracks.push(song(
                200 + artist * 3 + n,
                &format!("Jazz Artist {artist}"),
                &format!("Jazz Album {artist}"),
                "Jazz",
                1955 + artist as i32,
                0,
            ));
        }
    }
    for artist in 0..8 {
        for n in 0..3 {
            tracks.push(song(
                300 + artist * 3 + n,
                &format!("Classical Artist {artist}"),
                &format!("Classical Album {artist}"),
                "Classical",
                1800 + artist as i32,
                0,
            ));
        }
    }
    for album in 0..5 {
        for n in 0..5 {
            let rating = if n == 0 { 80 } else { 0 };
            tracks.push(song(
                400 + album * 5 + n,
                "Big Name",
                &format!("Big Album {album}"),
                "Pop",
                2000,
                rating,
            ));
        }
    }
    for artist in 0..10 {
        for n in 0..3 {
            tracks.push(song(
                450 + artist * 3 + n,
                &format!("Pop Artist {artist}"),
                &format!("Pop Album {artist}"),
                "Pop",
                2001,
                100,
            ));
        }
    }
    for artist in 0..5 {
        tracks.push(song(
            500 + artist,
            &format!("Folk Artist {artist}"),
            "Folk Album",
            "Folk",
            1970,
            0,
        ));
    }
    // Decoys in the Rock cluster.
    tracks.push(Track {
        kind: MediaKind::Podcast,
        ..song(600, "Rock Artist 0", "Rock Cast", "Rock", 1990, 100)
    });
    tracks.push(Track {
        kind: MediaKind::Audiobook,
        ..song(601, "Rock Artist 1", "Rock Book", "Rock", 1990, 100)
    });
    for n in 0..6 {
        tracks.push(Track {
            enabled: false,
            ..song(
                610 + n,
                &format!("Rock Artist {}", 2 + n),
                "Unchecked",
                "Rock",
                1990,
                100,
            )
        });
    }
    tracks
}

const DECOYS: [i64; 8] = [600, 601, 610, 611, 612, 613, 614, 615];

fn genre_of(library: &Library, id: i64) -> String {
    library.track(id).unwrap().unwrap().genre
}

/// No artist more than twice in any ten consecutive songs, and no album twice running.
fn assert_varied(library: &Library, ids: &[i64]) {
    let tracks: Vec<Track> = ids
        .iter()
        .map(|id| library.track(*id).unwrap().unwrap())
        .collect();
    for window in tracks
        .windows(10)
        .chain(std::iter::once(tracks.as_slice()).filter(|t| t.len() < 10))
    {
        for track in window {
            let same = window
                .iter()
                .filter(|other| other.artist == track.artist)
                .count();
            assert!(
                same <= 2,
                "{} appears {same} times in {:?}",
                track.artist,
                window.iter().map(|t| t.id).collect::<Vec<_>>()
            );
        }
    }
    for pair in tracks.windows(2) {
        assert_ne!(
            pair[0].album, pair[1].album,
            "{} then {}",
            pair[0].id, pair[1].id
        );
    }
}

#[test]
fn a_mix_stays_with_the_seeds_neighbourhood_and_leaves_the_seed_out() {
    let library = library_of(&catalogue());

    let mix = library.mix_for(&[100], 20).unwrap();

    assert_eq!(mix.len(), 20);
    assert!(!mix.contains(&100));
    assert!(
        mix.iter().all(|id| genre_of(&library, *id) == "Rock"),
        "pure Rock: {mix:?}"
    );
    assert!(
        mix.iter().all(|id| !DECOYS.contains(id)),
        "no podcasts, audiobooks or unchecked songs"
    );
    assert_eq!(mix.iter().collect::<HashSet<_>>().len(), 20, "no repeats");
    assert_varied(&library, &mix);
}

#[test]
fn mixes_are_deterministic() {
    let tracks = catalogue();
    let library = library_of(&tracks);
    let mut reversed = tracks.clone();
    reversed.reverse();
    let rebuilt = library_of(&reversed);

    let first = library.mix_for(&[100, 205], 30).unwrap();

    assert_eq!(first, library.mix_for(&[100, 205], 30).unwrap());
    assert_eq!(
        first,
        rebuilt.mix_for(&[100, 205], 30).unwrap(),
        "insertion order does not matter"
    );
    assert_eq!(library.mixes(6, 20).unwrap(), library.mixes(6, 20).unwrap());
    assert_eq!(library.mixes(6, 20).unwrap(), rebuilt.mixes(6, 20).unwrap());
}

#[test]
fn seeds_from_two_clusters_blend_them() {
    let library = library_of(&catalogue());

    let mix = library.mix_for(&[100, 200], 24).unwrap();

    let genres: HashSet<String> = mix.iter().map(|id| genre_of(&library, *id)).collect();
    assert_eq!(
        genres,
        HashSet::from(["Rock".to_owned(), "Jazz".to_owned()])
    );
    assert!(!mix.contains(&100) && !mix.contains(&200));
    assert_varied(&library, &mix);
}

#[test]
fn odd_seeds_and_sizes_give_short_or_empty_mixes_not_errors() {
    let library = library_of(&catalogue());

    assert!(library.mix_for(&[], 10).unwrap().is_empty());
    assert!(library.mix_for(&[100], 0).unwrap().is_empty());
    assert!(
        library.mix_for(&[9_999], 10).unwrap().is_empty(),
        "unknown seed"
    );
    assert!(
        library.mix_for(&[600], 10).unwrap().is_empty(),
        "a podcast is no seed"
    );
    assert_eq!(
        library.mix_for(&[100, 100, 9_999], 5).unwrap().len(),
        5,
        "repeats and unknowns are ignored"
    );
    // Only 4 other Folk songs exist, and a mix is never padded with strangers.
    let folk = library.mix_for(&[500], 20).unwrap();
    assert_eq!(folk.len(), 4);
    assert!(folk.iter().all(|id| genre_of(&library, *id) == "Folk"));
    assert!(
        Library::open_in_memory()
            .unwrap()
            .mix_for(&[1], 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn songs_played_in_one_session_link_across_genres_and_distant_plays_do_not() {
    let mut library = library_of(&catalogue());
    for visit in 0..4 {
        let start = T0 + visit * DAY;
        library.record_play(100, start).unwrap();
        library.record_play(200, start + 120).unwrap();
        // A Classical song played 31 minutes later is a different session.
        library.record_play(300, start + 120 + 31 * 60).unwrap();
    }

    let mix = library.mix_for(&[100], 40).unwrap();

    assert!(
        mix.contains(&200),
        "played right after the seed, four times"
    );
    assert!(!mix.contains(&300), "31 minutes apart is a new session");
    assert!(!mix.contains(&201), "its Jazz neighbours are not linked");
}

#[test]
fn a_gap_just_under_thirty_minutes_still_links() {
    let mut library = library_of(&catalogue());
    library.record_play(100, T0).unwrap();
    library.record_play(200, T0 + 29 * 60).unwrap();

    assert!(library.mix_for(&[100], 40).unwrap().contains(&200));
}

#[test]
fn songs_sharing_a_playlist_with_a_seed_link() {
    let mut library = library_of(&catalogue());
    let playlist = library
        .create_playlist("Road trip", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library
        .add_to_playlist(playlist, &[100, 205, 305], None)
        .unwrap();

    let mix = library.mix_for(&[100], 40).unwrap();

    assert!(mix.contains(&205) && mix.contains(&305));
    assert!(
        !mix.contains(&206),
        "only the listed songs, not their genre"
    );
}

#[test]
fn artists_credited_together_link() {
    let mut tracks = catalogue();
    let solo = Track {
        genre: "Fusion".into(),
        ..song(701, "Fusion Guy", "Solo Album", "Fusion", 1990, 0)
    };
    tracks.push(solo.clone());
    let alone = library_of(&tracks);
    assert!(!alone.mix_for(&[100], 40).unwrap().contains(&701));

    // One song credits Rock Artist 0 together with Fusion Guy.
    tracks.push(song(
        700,
        "Rock Artist 0 feat. Fusion Guy",
        "Duets",
        "Fusion",
        1990,
        0,
    ));
    let together = library_of(&tracks);
    let mix = together.mix_for(&[100], 40).unwrap();
    assert!(mix.contains(&700), "shares the primary artist");
    assert!(mix.contains(&701), "the collaborator's own songs follow");
}

#[test]
fn year_and_rating_order_songs_that_are_already_linked() {
    let tracks = vec![
        song(800, "Seed Artist", "Seed Album", "Blues", 1970, 0),
        song(801, "Artist A", "Album A", "Blues", 1970, 0),
        song(802, "Artist B", "Album B", "Blues", 1970, 100),
        song(803, "Artist C", "Album C", "Blues", 1940, 0),
        song(804, "Artist D", "Album D", "Blues", 1975, 0),
        // Highly rated, near in year, but nothing links it to the seed.
        song(805, "Artist E", "Album E", "Zydeco", 1970, 100),
    ];
    let library = library_of(&tracks);

    let mix = library.mix_for(&[800], 10).unwrap();

    assert_eq!(
        mix,
        [802, 801, 804, 803],
        "rating, then nearness in year; the unlinked song never qualifies"
    );
}

#[test]
fn songs_never_repeat_an_album_back_to_back_or_crowd_one_artist() {
    // One artist with many songs among a few others forces the rules to bite.
    let mut tracks = Vec::new();
    for n in 0..30 {
        tracks.push(song(
            900 + n,
            "Prolific",
            &format!("Prolific Album {}", n % 2),
            "Funk",
            1980,
            100,
        ));
    }
    for artist in 0..6 {
        for n in 0..2 {
            tracks.push(song(
                1000 + artist * 2 + n,
                &format!("Funk Artist {artist}"),
                &format!("Funk Album {artist}"),
                "Funk",
                1980,
                0,
            ));
        }
    }
    let library = library_of(&tracks);

    let mix = library.mix_for(&[1000], 30).unwrap();

    assert_varied(&library, &mix);
    let prolific = mix.iter().filter(|id| (900..930).contains(*id)).count();
    assert!(prolific <= 6, "two per ten, three tens at most: {prolific}");
}

#[test]
fn generated_mixes_are_pure_named_ordered_and_varied() {
    let library = library_of(&catalogue());

    let mixes = library.mixes(10, 20).unwrap();

    let keys: Vec<&str> = mixes.iter().map(|m| m.key.as_str()).collect();
    let names: Vec<&str> = mixes.iter().map(|m| m.name.as_str()).collect();
    // Clusters in order of size: Pop 55, Rock 36, Jazz 30, Big Name 25, Classical 24.
    assert_eq!(
        keys,
        [
            "genre:pop",
            "genre:rock",
            "genre:jazz",
            "artist:big name",
            "genre:classical"
        ]
    );
    assert_eq!(
        names,
        [
            "Pop Mix",
            "Rock Mix",
            "Jazz Mix",
            "Big Name & More",
            "Classical Mix"
        ]
    );
    for mix in &mixes {
        assert_eq!(mix.track_ids.len(), 20, "{}", mix.name);
        assert_eq!(mix.track_ids.iter().collect::<HashSet<_>>().len(), 20);
        assert!(
            mix.track_ids.iter().all(|id| !DECOYS.contains(id)),
            "{}",
            mix.name
        );
        assert_varied(&library, &mix.track_ids);
        if let Some(genre) = mix.key.strip_prefix("genre:") {
            assert!(
                mix.track_ids
                    .iter()
                    .all(|id| genre_of(&library, *id).to_lowercase() == genre),
                "{} is pure",
                mix.name
            );
        }
    }
    let big_name = mixes.iter().find(|m| m.key == "artist:big name").unwrap();
    let by_artist = big_name
        .track_ids
        .iter()
        .filter(|id| library.track(**id).unwrap().unwrap().artist == "Big Name")
        .count();
    assert!(
        (1..=4).contains(&by_artist),
        "the anchor artist is present but not dominant: {by_artist}"
    );
}

#[test]
fn mix_artwork_comes_from_up_to_four_distinct_albums() {
    let library = library_of(&catalogue());

    for mix in library.mixes(10, 20).unwrap() {
        assert!(
            !mix.artwork_track_ids.is_empty() && mix.artwork_track_ids.len() <= 4,
            "{}",
            mix.name
        );
        let albums: Vec<String> = mix
            .artwork_track_ids
            .iter()
            .map(|id| {
                assert!(mix.track_ids.contains(id), "{}", mix.name);
                library.track(*id).unwrap().unwrap().album
            })
            .collect();
        assert_eq!(
            albums.iter().collect::<HashSet<_>>().len(),
            albums.len(),
            "{albums:?}"
        );
        assert_eq!(
            mix.artwork_track_ids[0], mix.track_ids[0],
            "the mix's first song leads"
        );
    }
}

#[test]
fn clusters_too_small_for_the_size_are_skipped_and_count_caps_the_result() {
    let library = library_of(&catalogue());

    assert_eq!(library.mixes(2, 20).unwrap().len(), 2);
    assert_eq!(
        library
            .mixes(2, 20)
            .unwrap()
            .iter()
            .map(|m| m.key.as_str())
            .collect::<Vec<_>>(),
        ["genre:pop", "genre:rock"]
    );
    assert!(library.mixes(10, 100).unwrap().is_empty());
    assert!(library.mixes(0, 20).unwrap().is_empty());
    assert!(library.mixes(10, 0).unwrap().is_empty());
    assert!(
        Library::open_in_memory()
            .unwrap()
            .mixes(10, 20)
            .unwrap()
            .is_empty()
    );
    // Folk has five songs; it only mixes at size five or less.
    assert!(
        library
            .mixes(10, 5)
            .unwrap()
            .iter()
            .any(|m| m.key == "genre:folk")
    );
}

#[test]
fn unchecking_songs_removes_them_from_mixes() {
    let mut library = library_of(&catalogue());
    let before: Mix = library
        .mixes(10, 20)
        .unwrap()
        .into_iter()
        .find(|m| m.key == "genre:jazz")
        .unwrap();

    library.set_enabled(&before.track_ids, false).unwrap();

    let after = library.mixes(10, 20).unwrap();
    assert!(
        after
            .iter()
            .all(|m| m.track_ids.iter().all(|id| !before.track_ids.contains(id)))
    );
    assert!(
        after.iter().all(|m| m.key != "genre:jazz"),
        "ten songs left cannot fill twenty"
    );
}

#[test]
fn mixes_json_uses_camel_case() {
    let library = library_of(&catalogue());

    let json = serde_json::to_value(&library.mixes(1, 20).unwrap()[0]).unwrap();

    assert_eq!(json["key"], "genre:pop");
    assert!(json["trackIds"].is_array() && json["artworkTrackIds"].is_array());
}

/// Timing check kept out of the default run because debug builds are far
/// slower. Run with:
/// `cargo test -p unsub-library --release --test library -- --ignored mixes_stay`
#[test]
#[ignore = "timing check; needs --release"]
fn mixes_stay_fast_at_a_hundred_thousand_songs() {
    let genres = [
        "Rock", "Pop", "Jazz", "Blues", "Folk", "Soul", "Funk", "Metal", "Punk", "Techno",
    ];
    let tracks: Vec<Track> = (0..100_000_i64)
        .map(|i| Track {
            rating: (i % 6 * 20) as u8,
            play_count: (i % 40) as u32,
            ..song(
                i + 1,
                &format!("Artist {}", i % 3_000),
                &format!("Album {}", i % 9_000),
                genres[(i % 3_000 % 10) as usize],
                1960 + (i % 60) as i32,
                0,
            )
        })
        .collect();
    let mut library = library_of(&tracks);
    for n in 0..20_000_i64 {
        library
            .record_play(1 + n * 3 % 100_000, T0 + n * 90)
            .unwrap();
    }
    let playlist = library
        .create_playlist("Big", PlaylistKind::Regular, None)
        .unwrap()
        .id;
    library
        .add_to_playlist(playlist, &(1..=2_000).collect::<Vec<_>>(), None)
        .unwrap();

    let started = Instant::now();
    let mixes = library.mixes(12, 50).unwrap();
    let mixes_time = started.elapsed();
    let started = Instant::now();
    let more = library.mix_for(&[1, 2, 3], 50).unwrap();
    let mix_for_time = started.elapsed();

    println!("mixes(12, 50): {mixes_time:?}, mix_for: {mix_for_time:?}");
    assert_eq!(mixes.len(), 12);
    assert_eq!(more.len(), 50);
    assert!(mixes_time.as_millis() < 300, "took {mixes_time:?}");
}
