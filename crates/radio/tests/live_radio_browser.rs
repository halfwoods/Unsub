//! Against the real Radio Browser and a real station. Needs the network, so these only run when
//! asked: `cargo test -p antistreaming-radio -- --ignored`.
//!
//! They stay read-only: no clicks are counted, so running them does not skew any station's
//! popularity.

use std::io::Read;

use antistreaming_radio::{Directory, GENRES, genre, open};

#[test]
#[ignore = "needs the network"]
fn the_directory_lists_genres_searches_and_finds_a_playable_station() {
    let directory = Directory::new();

    let tags = directory.tags().expect("tags");
    assert!(tags.len() > 50, "{} tags", tags.len());
    assert!(
        tags.windows(2)
            .all(|pair| pair[0].station_count >= pair[1].station_count)
    );

    let jazz = genre("Jazz").expect("the Jazz genre");
    let page = directory
        .stations_by_genre(jazz, 0, 20)
        .expect("jazz stations");
    assert!(page.len() >= 10, "{} jazz stations", page.len());
    assert!(page.windows(2).all(|pair| pair[0].votes >= pair[1].votes));
    assert!(
        page.iter()
            .all(|station| station.url.starts_with("http") && !station.name.is_empty())
    );
    let next = directory
        .stations_by_genre(jazz, 20, 20)
        .expect("the second page");
    assert!(
        next.iter()
            .all(|station| page.iter().all(|seen| seen.uuid != station.uuid))
    );

    // Every genre should have something in it, or the Radio tab would show empty folders.
    for genre in GENRES {
        // A volunteer-run server: no need to hurry.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let stations = directory
            .stations_by_genre(genre, 0, 3)
            .unwrap_or_else(|error| panic!("{}: {error}", genre.name));
        assert!(!stations.is_empty(), "{} is empty", genre.name);
    }

    let found = directory.search("BBC Radio", 5).expect("search");
    assert!(!found.is_empty());
}

#[test]
#[ignore = "needs the network"]
fn a_popular_station_opens_and_plays_audio_bytes() {
    let directory = Directory::new();
    let stations = directory
        .stations_by_genre(genre("Pop").expect("Pop"), 0, 10)
        .expect("pop stations");
    let plain: Vec<_> = stations.iter().filter(|station| !station.is_hls).collect();

    // Stations come and go; one of the top ten should be on the air.
    let mut last_error = String::new();
    for station in plain {
        match open(&station.url) {
            Ok(mut live) => {
                let mut audio = vec![0u8; 64 * 1024];
                live.read_exact(&mut audio).expect("64 KiB of audio");
                println!("{} -> {:?}, {:?}", station.name, live.format(), live.info());
                return;
            }
            Err(error) => last_error = format!("{}: {error}", station.name),
        }
    }
    panic!("none of the top stations opened; the last said {last_error}");
}
