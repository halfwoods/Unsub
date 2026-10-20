//! Syncing the whole library through `search3`, the way Symfonium and our own remote client do: an
//! empty query, paged by count and offset, until a page comes back short.

mod common;

use common::*;

/// Every song in the fake library, in the order it serves them.
const SONGS: [&str; 10] = [
    "s-1", "s-2", "s-3", "s-4", "s-5", "s-6", "s-7", "s-9", "s-10", "s-8",
];
const ARTISTS: [&str; 3] = ["ar-1", "ar-2", "ar-3"];
const ALBUMS: [&str; 3] = ["al-1", "al-2", "al-3"];

fn ids(body: &serde_json::Value, kind: &str) -> Vec<String> {
    body["searchResult3"][kind]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["id"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// One `search3` request with an empty query.
fn page(h: &Harness, query: &str, paging: &str) -> serde_json::Value {
    h.json("search3", &format!("query={query}&{paging}"))
}

#[test]
fn songs_page_through_the_whole_library_and_then_run_out() {
    let h = Harness::start();
    let mut seen = Vec::new();
    let mut sizes = Vec::new();
    for offset in (0..).step_by(3).take(6) {
        let body = page(
            &h,
            "",
            &format!("artistCount=0&albumCount=0&songCount=3&songOffset={offset}"),
        );
        let songs = ids(&body, "song");
        sizes.push(songs.len());
        seen.extend(songs);
        // A kind that was not asked for is not sent.
        assert!(ids(&body, "artist").is_empty() && ids(&body, "album").is_empty());
    }
    assert_eq!(sizes, [3, 3, 3, 1, 0, 0]);
    assert_eq!(seen, SONGS);
}

#[test]
fn the_literal_pair_of_quotes_some_clients_send_means_the_same() {
    let h = Harness::start();
    for query in ["", "%22%22", "%22%22&artistCount=0&albumCount=0"] {
        let mut seen = Vec::new();
        for offset in (0..).step_by(4).take(4) {
            seen.extend(ids(
                &page(
                    &h,
                    query,
                    &format!("artistCount=0&albumCount=0&songCount=4&songOffset={offset}"),
                ),
                "song",
            ));
        }
        assert_eq!(seen, SONGS, "{query:?}");
    }
    // The source saw an empty query every time, never the quotes.
    let calls = h.source.calls.lock().unwrap();
    assert!(calls.search.iter().all(|call| call.0.is_empty()));
}

#[test]
fn a_page_that_ends_exactly_at_the_end_is_followed_by_an_empty_one() {
    let h = Harness::start();
    let sizes: Vec<usize> = [0, 5, 10]
        .iter()
        .map(|offset| {
            ids(
                &page(
                    &h,
                    "",
                    &format!("artistCount=0&albumCount=0&songCount=5&songOffset={offset}"),
                ),
                "song",
            )
            .len()
        })
        .collect();
    assert_eq!(sizes, [5, 5, 0]);
}

#[test]
fn artists_and_albums_page_the_same_way() {
    let h = Harness::start();
    let mut artists = Vec::new();
    let mut albums = Vec::new();
    let mut sizes = Vec::new();
    for offset in (0..).step_by(2).take(3) {
        let body = page(
            &h,
            "",
            &format!(
                "artistCount=2&artistOffset={offset}&albumCount=2&albumOffset={offset}&songCount=0"
            ),
        );
        sizes.push((
            ids(&body, "artist").len(),
            ids(&body, "album").len(),
            ids(&body, "song").len(),
        ));
        artists.extend(ids(&body, "artist"));
        albums.extend(ids(&body, "album"));
    }
    assert_eq!(sizes, [(2, 2, 0), (1, 1, 0), (0, 0, 0)]);
    assert_eq!(artists, ARTISTS);
    assert_eq!(albums, ALBUMS);
}

#[test]
fn each_kind_has_its_own_offset_in_one_request() {
    let h = Harness::start();
    let body = page(
        &h,
        "",
        "artistCount=5&artistOffset=2&albumCount=1&albumOffset=1&songCount=4&songOffset=8",
    );
    assert_eq!(ids(&body, "artist"), ["ar-3"]);
    assert_eq!(ids(&body, "album"), ["al-2"]);
    assert_eq!(ids(&body, "song"), ["s-10", "s-8"]);
    let call = h.source.calls.lock().unwrap().search.pop().unwrap();
    assert_eq!(call, (String::new(), (5, 2), (1, 1), (4, 8)));
}

#[test]
fn an_offset_far_beyond_the_end_is_an_empty_result_not_an_error() {
    let h = Harness::start();
    let body = page(
        &h,
        "",
        "artistOffset=1000000&albumOffset=1000000&songOffset=1000000",
    );
    assert!(
        ids(&body, "artist").is_empty()
            && ids(&body, "album").is_empty()
            && ids(&body, "song").is_empty()
    );
    // In XML the result is simply an empty element.
    let doc = xml_ok(&h.get(
        "search3",
        "query=&artistOffset=1000&albumOffset=1000&songOffset=1000",
    ));
    assert_eq!(
        child(doc.root_element(), "searchResult3")
            .children()
            .filter(|c| c.is_element())
            .count(),
        0
    );
}

#[test]
fn a_client_that_loops_until_a_page_comes_back_short_gets_every_item_once() {
    let h = Harness::start();
    // 500 at a time is what clients ask for; the library here is smaller, so it takes one page.
    for size in [1, 3, 4, 7, 10, 11, 500] {
        let mut seen = Vec::new();
        let mut offset = 0;
        loop {
            let body = page(
                &h,
                "",
                &format!("artistCount=0&albumCount=0&songCount={size}&songOffset={offset}"),
            );
            let songs = ids(&body, "song");
            let short = songs.len() < size;
            offset += songs.len();
            seen.extend(songs);
            if short {
                break;
            }
        }
        assert_eq!(seen, SONGS, "page size {size}");
    }
}

#[test]
fn asking_for_everything_in_one_request_gets_everything_however_big_the_number() {
    let h = Harness::start();
    for count in ["500", "20000", "99999"] {
        let body = page(
            &h,
            "",
            &format!("artistCount={count}&albumCount={count}&songCount={count}"),
        );
        assert_eq!(
            (
                ids(&body, "artist").len(),
                ids(&body, "album").len(),
                ids(&body, "song").len()
            ),
            (3, 3, 10),
            "{count}"
        );
    }
    // Only an absurd request is trimmed, and well above any real library.
    page(&h, "", "songCount=99999999999");
    let call = h.source.calls.lock().unwrap().search.pop().unwrap();
    assert_eq!(call.3, (100_000, 0));
    let call = h.source.calls.lock().unwrap().search[2].clone();
    assert_eq!(call.3, (99_999, 0));
}

#[test]
fn without_counts_a_page_is_twenty_of_each_kind() {
    let h = Harness::start();
    let body = page(&h, "", "");
    assert_eq!(
        (
            ids(&body, "artist").len(),
            ids(&body, "album").len(),
            ids(&body, "song").len()
        ),
        (3, 3, 10)
    );
    let call = h.source.calls.lock().unwrap().search.pop().unwrap();
    assert_eq!(call, (String::new(), (20, 0), (20, 0), (20, 0)));
}

#[test]
fn the_older_search2_pages_the_same_way() {
    let h = Harness::start();
    let mut seen = Vec::new();
    for offset in (0..).step_by(6).take(3) {
        let body = h.json(
            "search2",
            &format!("query=%22%22&artistCount=0&albumCount=0&songCount=6&songOffset={offset}"),
        );
        seen.extend(
            body["searchResult2"]["song"]
                .as_array()
                .map(|songs| {
                    songs
                        .iter()
                        .map(|s| s["id"].as_str().unwrap().to_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        );
    }
    assert_eq!(seen, SONGS);
}

#[test]
fn a_nonempty_query_pages_too() {
    let h = Harness::start();
    let first = ids(
        &page(
            &h,
            "beatles",
            "artistCount=0&albumCount=1&albumOffset=0&songCount=3&songOffset=0",
        ),
        "song",
    );
    let second = ids(
        &page(
            &h,
            "beatles",
            "artistCount=0&albumCount=1&albumOffset=1&songCount=3&songOffset=3",
        ),
        "song",
    );
    assert_eq!((first.len(), second.len()), (3, 3));
    assert!(first.iter().all(|id| !second.contains(id)));
}

#[test]
fn a_failing_source_stops_the_sync_with_an_error_not_a_short_page() {
    // A truncated page would read as the end of the library, so a failure must be loud.
    let h = Harness::start();
    let (code, message) = error_of(&h.get("search3", "query=broken"));
    assert_eq!(code, 0);
    assert_eq!(message, "database is locked");
}
