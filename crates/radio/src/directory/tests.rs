use std::time::Duration;

use serde_json::json;

use super::*;
use crate::testing::{Server, head, not_found, ok};

const JAZZ: &str = include_str!("../../tests/fixtures/radiobrowser_stations_jazz.json");
const HLS: &str = include_str!("../../tests/fixtures/radiobrowser_stations_hls.json");
const TAGS: &str = include_str!("../../tests/fixtures/radiobrowser_tags.json");
const SERVERS: &str = include_str!("../../tests/fixtures/radiobrowser_servers.json");

fn json_ok(stream: &mut std::net::TcpStream, body: &str) {
    ok(stream, "application/json", body.as_bytes());
}

fn station_json(uuid: &str, name: &str, votes: u64) -> serde_json::Value {
    json!({
        "stationuuid": uuid, "name": name, "url": format!("http://{uuid}.example.com/stream"),
        "url_resolved": "", "homepage": "", "favicon": "", "tags": "jazz", "country": "France",
        "codec": "MP3", "bitrate": 128, "hls": 0, "votes": votes
    })
}

fn directory(server: &Server) -> Directory {
    Directory::with_mirrors(vec![server.base()])
}

/// Which `tag=` values a request asked for, and the other query parameters.
fn query_of(path: &str) -> Vec<(String, String)> {
    Url::parse(&format!("http://x{path}"))
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn param<'a>(query: &'a [(String, String)], name: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn recorded_stations_decode_into_the_station_type() {
    let server = Server::start(|_, stream| json_ok(stream, JAZZ));
    let stations = directory(&server).search("jazz", 5).unwrap();
    assert_eq!(stations.len(), 5);
    let first = &stations[0];
    assert_eq!(first.uuid, "d1a54d2e-623e-4970-ab11-35f7b56c5ec3");
    assert_eq!(first.name, "Classic Vinyl HD");
    assert_eq!(first.url, "https://icecast.walmradio.com:8443/classic");
    assert_eq!(
        (
            first.codec.as_str(),
            first.bitrate_kbps,
            first.is_hls,
            first.votes
        ),
        ("MP3", 320, false, 308_109)
    );
    assert_eq!(first.country, "The United States Of America");
    assert_eq!(first.homepage, "https://walmradio.com/classic");
    assert!(first.tags.contains(&"big band".to_owned()) && first.tags.contains(&"jazz".to_owned()));
    assert!(
        first
            .tags
            .iter()
            .all(|tag| tag == tag.trim() && !tag.is_empty())
    );
    assert!(
        stations
            .windows(2)
            .all(|pair| pair[0].votes >= pair[1].votes)
    );
}

#[test]
fn hls_stations_are_marked_and_unknown_bit_rates_are_zero() {
    let server = Server::start(|_, stream| json_ok(stream, HLS));
    let stations = directory(&server).search("bbc", 5).unwrap();
    assert!(
        stations
            .iter()
            .all(|station| station.is_hls && station.url.ends_with(".m3u8"))
    );
    assert_eq!(
        (stations[0].codec.as_str(), stations[0].bitrate_kbps),
        ("UNKNOWN", 0)
    );
}

#[test]
fn messy_entries_are_cleaned_and_unplayable_ones_dropped() {
    let body = json!([
        { "stationuuid": "a", "name": " Padded Name\n", "url": " http://a.example.com/x ", "url_resolved": "", "tags": "Rock, ,POP ,", "hls": 1 },
        { "stationuuid": "b", "name": "No Address", "url": "", "url_resolved": "" },
        { "stationuuid": "c", "name": "Prefers Resolved", "url": "http://old.example.com/x.pls", "url_resolved": "http://new.example.com/x" },
        { "stationuuid": "d" }
    ]);
    let server = Server::start(move |_, stream| json_ok(stream, &body.to_string()));
    let stations = directory(&server).search("x", 10).unwrap();
    let seen: Vec<_> = stations
        .iter()
        .map(|station| {
            (
                station.uuid.as_str(),
                station.name.as_str(),
                station.url.as_str(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("a", "Padded Name", "http://a.example.com/x"),
            ("c", "Prefers Resolved", "http://new.example.com/x")
        ]
    );
    assert_eq!(stations[0].tags, ["rock", "pop"]);
    assert!(stations[0].is_hls);
    assert_eq!((stations[0].bitrate_kbps, stations[0].votes), (0, 0));
}

#[test]
fn a_search_asks_for_the_most_voted_working_stations_with_the_name_encoded() {
    let server = Server::start(|_, stream| json_ok(stream, "[]"));
    assert!(
        directory(&server)
            .search(" Rock & Roll FM ", 25)
            .unwrap()
            .is_empty()
    );
    let request = &server.requests()[0];
    assert!(request.path.starts_with("/json/stations/search?"));
    assert!(
        request.path.contains("name=Rock+%26+Roll+FM"),
        "{}",
        request.path
    );
    let query = query_of(&request.path);
    assert_eq!(param(&query, "name"), Some("Rock & Roll FM"));
    assert_eq!(param(&query, "order"), Some("votes"));
    assert_eq!(param(&query, "reverse"), Some("true"));
    assert_eq!(param(&query, "hidebroken"), Some("true"));
    assert_eq!(param(&query, "limit"), Some("25"));
    assert!(
        request
            .header("user-agent")
            .is_some_and(|agent| agent.starts_with("Unsub/"))
    );
}

#[test]
fn a_genre_merges_its_tags_by_votes_and_removes_duplicates() {
    let server = Server::start(|request, stream| {
        let query = query_of(&request.path);
        let list = match param(&query, "tag") {
            Some("jazz") => json!([
                station_json("a", "A", 900),
                station_json("c", "C", 500),
                station_json("e", "E", 100)
            ]),
            Some("smooth jazz") => json!([
                station_json("b", "B", 700),
                station_json("c", "C", 500),
                station_json("d", "D", 300)
            ]),
            _ => json!([]),
        };
        json_ok(stream, &list.to_string());
    });
    let jazz = genre("Jazz").unwrap();
    let all = directory(&server).stations_by_genre(jazz, 0, 100).unwrap();
    let order: Vec<_> = all.iter().map(|station| station.uuid.as_str()).collect();
    assert_eq!(order, ["a", "b", "c", "d", "e"]);
    // One request per tag, each asking for exact tag matches.
    assert_eq!(server.requests().len(), jazz.tags.len());
    let tags: Vec<_> = server
        .requests()
        .iter()
        .map(|request| query_of(&request.path))
        .collect();
    assert!(
        tags.iter()
            .all(|query| param(query, "tagExact") == Some("true")
                && param(query, "hidebroken") == Some("true"))
    );
    let mut asked: Vec<_> = tags
        .iter()
        .filter_map(|query| param(query, "tag"))
        .collect();
    asked.sort_unstable();
    let mut expected = jazz.tags.to_vec();
    expected.sort_unstable();
    assert_eq!(asked, expected);
}

#[test]
fn genre_pages_slice_the_merged_list_and_ask_each_tag_for_enough() {
    let server = Server::start(|request, stream| {
        let query = query_of(&request.path);
        let limit: usize = param(&query, "limit")
            .and_then(|limit| limit.parse().ok())
            .unwrap();
        let offset: usize = param(&query, "offset")
            .and_then(|offset| offset.parse().ok())
            .unwrap();
        assert_eq!(offset, 0, "every tag list starts at its top");
        let own = match param(&query, "tag") {
            Some("jazz") => (0..10)
                .map(|n| station_json(&format!("j{n}"), "J", 1000 - n * 2))
                .collect::<Vec<_>>(),
            Some("bebop") => (0..10)
                .map(|n| station_json(&format!("b{n}"), "B", 999 - n * 2))
                .collect(),
            _ => Vec::new(),
        };
        json_ok(
            stream,
            &json!(own.into_iter().take(limit).collect::<Vec<_>>()).to_string(),
        );
    });
    let jazz = genre("Jazz").unwrap();
    let dir = directory(&server);
    let page = |offset, limit| {
        dir.stations_by_genre(jazz, offset, limit)
            .unwrap()
            .into_iter()
            .map(|station| station.uuid)
            .collect::<Vec<_>>()
    };
    // Interleaved by votes: j0 (1000), b0 (999), j1 (998), b1 (997), ...
    assert_eq!(page(0, 4), ["j0", "b0", "j1", "b1"]);
    assert_eq!(page(4, 4), ["j2", "b2", "j3", "b3"]);
    assert_eq!(page(16, 10), ["j8", "b8", "j9", "b9"]);
    assert!(page(100, 10).is_empty());
    let deepest = server
        .requests()
        .iter()
        .filter_map(|request| {
            param(&query_of(&request.path), "limit").and_then(|l| l.parse::<u32>().ok())
        })
        .max();
    assert_eq!(deepest, Some(110));
}

#[test]
fn a_genre_survives_one_of_its_tags_failing_but_not_all_of_them() {
    let server = Server::start(|request, stream| {
        let query = query_of(&request.path);
        if param(&query, "tag") == Some("jazz") {
            json_ok(stream, &json!([station_json("a", "A", 5)]).to_string());
        } else {
            not_found(stream);
        }
    });
    let jazz = genre("Jazz").unwrap();
    assert_eq!(
        directory(&server)
            .stations_by_genre(jazz, 0, 10)
            .unwrap()
            .len(),
        1
    );

    let broken = Server::start(|_, stream| not_found(stream));
    assert!(matches!(
        directory(&broken).stations_by_genre(jazz, 0, 10),
        Err(Error::NoMirror(_))
    ));
}

#[test]
fn tags_are_decoded_from_the_recorded_answer_and_cached() {
    let server = Server::start(|_, stream| json_ok(stream, TAGS));
    let dir = directory(&server);
    let tags = dir.tags().unwrap();
    assert_eq!(tags.len(), 40);
    assert_eq!(
        (tags[0].name.as_str(), tags[0].station_count),
        ("pop", 6147)
    );
    assert!(
        tags.windows(2)
            .all(|pair| pair[0].station_count >= pair[1].station_count)
    );
    let query = query_of(&server.requests()[0].path);
    assert_eq!(
        (
            param(&query, "order"),
            param(&query, "reverse"),
            param(&query, "hidebroken")
        ),
        (Some("stationcount"), Some("true"), Some("true"))
    );

    assert_eq!(dir.tags().unwrap(), tags);
    assert_eq!(
        server.requests().len(),
        1,
        "the second call is answered from memory"
    );
}

#[test]
fn tags_are_fetched_again_once_they_are_stale() {
    let server = Server::start(|_, stream| json_ok(stream, TAGS));
    let dir = directory(&server).with_tags_ttl(Duration::ZERO);
    dir.tags().unwrap();
    dir.tags().unwrap();
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn the_tag_cache_file_survives_a_restart_until_it_goes_stale() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("tags.json");
    let server = Server::start(|_, stream| json_ok(stream, TAGS));
    let first = directory(&server).with_cache_file(&file).tags().unwrap();
    assert!(file.exists());

    // A new process: nothing in memory, the file stands in.
    let restarted = directory(&server).with_cache_file(&file);
    assert_eq!(restarted.tags().unwrap(), first);
    assert_eq!(server.requests().len(), 1);

    // A day later it is read but no longer believed.
    let ancient = TagsCache {
        fetched_at: unix_now() - 2 * 24 * 60 * 60,
        tags: first.clone(),
    };
    std::fs::write(&file, serde_json::to_vec(&ancient).unwrap()).unwrap();
    directory(&server).with_cache_file(&file).tags().unwrap();
    assert_eq!(server.requests().len(), 2);

    // And a damaged file is just a cache miss.
    std::fs::write(&file, b"{ not json").unwrap();
    directory(&server).with_cache_file(&file).tags().unwrap();
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn the_next_mirror_is_tried_when_one_fails_and_the_working_one_is_remembered() {
    let broken = Server::start(|_, stream| {
        let _ = head(
            stream,
            "HTTP/1.1 503 Service Unavailable",
            &[("Content-Length", "0")],
        );
    });
    let working = Server::start(|_, stream| json_ok(stream, "[]"));
    let dir = Directory::with_mirrors(vec![broken.base(), working.base()]);
    dir.search("a", 5).unwrap();
    dir.search("b", 5).unwrap();
    dir.search("c", 5).unwrap();
    assert_eq!(
        broken.requests().len(),
        1,
        "after the first failover the working mirror goes first"
    );
    assert_eq!(working.requests().len(), 3);
}

#[test]
fn with_no_working_mirror_the_error_says_why() {
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let error = Directory::with_mirrors(vec![dead])
        .search("a", 5)
        .unwrap_err();
    assert!(
        matches!(&error, Error::NoMirror(reason) if reason.contains("could not reach")),
        "{error}"
    );
    let none = Directory::with_mirrors(Vec::new())
        .search("a", 5)
        .unwrap_err();
    assert!(matches!(none, Error::NoMirror(_)));
}

#[test]
fn an_answer_that_is_not_json_is_an_error_not_a_panic() {
    let server = Server::start(|_, stream| json_ok(stream, "<html>maintenance</html>"));
    assert!(matches!(
        directory(&server).search("a", 5),
        Err(Error::Response { .. })
    ));
}

#[test]
fn clicks_are_counted_through_the_url_endpoint() {
    let server = Server::start(|_, stream| {
        json_ok(
            stream,
            r#"{"ok":true,"message":"retrieved station url","stationuuid":"d1a54d2e-623e-4970-ab11-35f7b56c5ec3","name":"x","url":"http://x"}"#,
        );
    });
    directory(&server)
        .count_click("d1a54d2e-623e-4970-ab11-35f7b56c5ec3")
        .unwrap();
    assert_eq!(
        server.requests()[0].path,
        "/json/url/d1a54d2e-623e-4970-ab11-35f7b56c5ec3"
    );

    let refusing =
        Server::start(|_, stream| json_ok(stream, r#"{"ok":false,"message":"station not found"}"#));
    let error = directory(&refusing)
        .count_click("d1a54d2e-623e-4970-ab11-35f7b56c5ec3")
        .unwrap_err();
    assert!(error.to_string().contains("station not found"), "{error}");
}

#[test]
fn only_station_ids_may_go_into_the_click_path() {
    let server = Server::start(|_, stream| json_ok(stream, r#"{"ok":true}"#));
    let dir = directory(&server);
    for bad in ["", "../json/tags", "abc/def", "a b", "abc?x=1", "zzz"] {
        assert!(dir.count_click(bad).is_err(), "{bad:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn the_published_server_list_becomes_https_mirrors_without_repeats() {
    let server = Server::start(|_, stream| json_ok(stream, SERVERS));
    let dir = Directory::new();
    dir.mirrors.lock().unwrap().discovery = server.base();
    assert_eq!(dir.discover(), ["https://de1.api.radio-browser.info"]);

    let noisy = Server::start(|_, stream| {
        json_ok(
            stream,
            r#"[{"name":"a.example.com"},{"name":" "},{"name":"b.example.com"},{"name":"a.example.com"}]"#,
        )
    });
    dir.mirrors.lock().unwrap().discovery = noisy.base();
    assert_eq!(
        dir.discover(),
        ["https://a.example.com", "https://b.example.com"]
    );
}

#[test]
fn known_mirrors_stand_in_when_the_server_list_cannot_be_read() {
    let broken = Server::start(|_, stream| json_ok(stream, "nonsense"));
    let dir = Directory::new();
    dir.mirrors.lock().unwrap().discovery = broken.base();
    assert_eq!(dir.discover(), FALLBACK_MIRRORS);
}

#[test]
fn discovery_runs_once_and_its_result_is_shuffled_but_complete() {
    let lister = Server::start(|_, stream| {
        json_ok(
            stream,
            r#"[{"name":"a.example.com"},{"name":"b.example.com"},{"name":"c.example.com"}]"#,
        )
    });
    let dir = Directory::new();
    dir.mirrors.lock().unwrap().discovery = lister.base();
    let bases = dir.bases();
    assert_eq!(bases.len(), 4);
    let mut found: Vec<_> = bases[..3].to_vec();
    found.sort();
    assert_eq!(
        found,
        [
            "https://a.example.com",
            "https://b.example.com",
            "https://c.example.com"
        ]
    );
    // The round-robin address is the last resort.
    assert_eq!(bases[3], lister.base());
    assert_eq!(dir.bases(), bases);
    assert_eq!(lister.requests().len(), 1);
}

#[test]
fn stations_know_which_genres_they_belong_to() {
    let server = Server::start(|_, stream| json_ok(stream, JAZZ));
    let stations = directory(&server).search("jazz", 5).unwrap();
    let names: Vec<_> = genres_of(&stations[0])
        .iter()
        .map(|genre| genre.name)
        .collect();
    assert!(
        names.contains(&"Jazz") && names.contains(&"Oldies") && names.contains(&"Easy Listening"),
        "{names:?}"
    );
    assert!(!names.contains(&"Metal"));
    let none = Station {
        tags: vec!["polka".into()],
        ..stations[0].clone()
    };
    assert!(genres_of(&none).is_empty());
}

#[test]
fn stations_serialise_for_the_interface_in_camel_case() {
    let server = Server::start(|_, stream| json_ok(stream, HLS));
    let station = directory(&server).search("bbc", 1).unwrap().remove(0);
    let value = serde_json::to_value(&station).unwrap();
    assert_eq!(value["isHls"], true);
    assert_eq!(value["bitrateKbps"], 0);
    assert!(value.get("stationuuid").is_none());
    assert_eq!(serde_json::from_value::<Station>(value).unwrap(), station);
}
