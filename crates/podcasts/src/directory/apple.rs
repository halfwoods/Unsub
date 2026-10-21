//! Apple's public podcast directory, which needs no key. Its per-category chart is only offered
//! in the older RSS-style JSON feed, so charts are read from there and the shows in them are
//! looked up in one batch for their feed addresses.

use serde::{Deserialize, Deserializer};

use super::{
    DEFAULT_COUNTRY, Directory, Listing, Listings, RESULT_LIMIT, Source, dedupe, image_url,
    system_country,
};
use crate::error::{Error, Result};
use crate::http::{self, Budget};

pub(super) struct AppleDirectory {
    agent: ureq::Agent,
}

impl Default for AppleDirectory {
    fn default() -> Self {
        Self {
            agent: http::agent(http::DEFAULT_STALL_TIMEOUT),
        }
    }
}

/// The two-letter storefront for the user's region, as the directory wants it in lower case.
fn country() -> String {
    system_country().unwrap_or_else(|| DEFAULT_COUNTRY.to_owned())
}

impl Directory for AppleDirectory {
    fn search(&self, term: &str) -> Result<Listings> {
        let term = term.trim();
        if term.is_empty() {
            return Ok(listings(Vec::new()));
        }
        let body = http::fetch_directory(
            &self.agent,
            "https://itunes.apple.com/search",
            &[
                ("media", "podcast"),
                ("entity", "podcast"),
                ("term", term),
                ("limit", RESULT_LIMIT),
                ("country", &country()),
            ],
            &[],
            Budget::PATIENT,
        )?;
        parse_search(&body).map(listings)
    }

    fn top(&self, category: Option<u32>) -> Result<Listings> {
        let country = country();
        let chart = http::fetch_directory(
            &self.agent,
            &chart_url(&country, category),
            &[],
            &[],
            Budget::PATIENT,
        )?;
        let ids = parse_chart(&chart)?;
        if ids.is_empty() {
            return Ok(listings(Vec::new()));
        }
        let joined = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let body = http::fetch_directory(
            &self.agent,
            "https://itunes.apple.com/lookup",
            &[
                ("id", &joined),
                ("entity", "podcast"),
                ("country", &country),
            ],
            &[],
            Budget::PATIENT,
        )?;
        parse_lookup(&body, &ids).map(listings)
    }

    fn source(&self) -> Source {
        Source::Apple
    }
}

fn listings(shows: Vec<Listing>) -> Listings {
    Listings {
        source: Source::Apple,
        shows,
    }
}

fn chart_url(country: &str, category: Option<u32>) -> String {
    let genre = category.map_or_else(String::new, |id| format!("/genre={id}"));
    format!("https://itunes.apple.com/{country}/rss/toppodcasts/limit={RESULT_LIMIT}{genre}/json")
}

#[derive(Deserialize)]
struct SearchReply {
    #[serde(default)]
    results: Vec<Found>,
}

/// One show in a search or lookup reply; either may carry fields the other lacks.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Found {
    collection_id: Option<u64>,
    feed_url: Option<String>,
    collection_name: Option<String>,
    artist_name: Option<String>,
    artwork_url100: Option<String>,
    artwork_url600: Option<String>,
    primary_genre_name: Option<String>,
    track_count: Option<u32>,
}

impl Found {
    /// `None` for a show with no feed address, which can't be subscribed to.
    fn into_listing(self) -> Option<Listing> {
        let feed_url = self.feed_url.filter(|url| !url.trim().is_empty())?;
        Some(Listing {
            title: self
                .collection_name
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| feed_url.clone()),
            feed_url,
            author: self.artist_name.unwrap_or_default(),
            // Rows are shown at about 50 pixels, so the 100-pixel image is sharp enough on a
            // high-density display and a twelfth of the download.
            artwork_url: image_url(self.artwork_url100).or_else(|| image_url(self.artwork_url600)),
            genre: self.primary_genre_name.unwrap_or_default(),
            episode_count: self.track_count,
        })
    }
}

fn parse_found(body: &[u8]) -> Result<Vec<Found>> {
    serde_json::from_slice::<SearchReply>(body)
        .map(|reply| reply.results)
        .map_err(|err| Error::Directory(err.to_string()))
}

pub(super) fn parse_search(body: &[u8]) -> Result<Vec<Listing>> {
    let listings = parse_found(body)?
        .into_iter()
        .filter_map(Found::into_listing)
        .collect();
    Ok(dedupe(listings))
}

/// A lookup answers in no particular order, so the listings are put back in the order of `ids`.
fn parse_lookup(body: &[u8], ids: &[u64]) -> Result<Vec<Listing>> {
    let mut found = parse_found(body)?;
    found.retain(|show| show.collection_id.is_some_and(|id| ids.contains(&id)));
    found.sort_by_key(|show| ids.iter().position(|id| Some(*id) == show.collection_id));
    Ok(found.into_iter().filter_map(Found::into_listing).collect())
}

#[derive(Deserialize)]
struct ChartReply {
    feed: ChartFeed,
}

#[derive(Deserialize)]
struct ChartFeed {
    #[serde(default, deserialize_with = "one_or_many")]
    entry: Vec<ChartEntry>,
}

#[derive(Deserialize)]
struct ChartEntry {
    id: ChartEntryId,
}

#[derive(Deserialize)]
struct ChartEntryId {
    attributes: ChartEntryIdAttributes,
}

#[derive(Deserialize)]
struct ChartEntryIdAttributes {
    #[serde(rename = "im:id")]
    id: String,
}

/// The chart is converted from XML, where one `<entry>` becomes an object and several become an
/// array.
fn one_or_many<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        Many(Vec<T>),
        One(T),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::Many(many) => many,
        OneOrMany::One(one) => vec![one],
    })
}

/// The show ids of a chart, most popular first.
fn parse_chart(body: &[u8]) -> Result<Vec<u64>> {
    let chart: ChartReply =
        serde_json::from_slice(body).map_err(|err| Error::Directory(err.to_string()))?;
    chart
        .feed
        .entry
        .iter()
        .map(|entry| {
            entry
                .id
                .attributes
                .id
                .parse()
                .map_err(|_| Error::Directory(format!("bad show id {}", entry.id.attributes.id)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = r#"{"resultCount": 6, "results": [
        {"wrapperType": "track", "kind": "podcast", "collectionId": 11,
         "collectionName": "Show One", "artistName": "Ann", "feedUrl": "https://one.example/rss",
         "artworkUrl100": "https://img.example/one100.jpg", "artworkUrl600": "https://img.example/one600.jpg",
         "primaryGenreName": "History", "trackCount": 120},
        {"collectionId": 12, "collectionName": "No Feed", "artistName": "Bob"},
        {"collectionId": 13, "collectionName": "Only Large Art", "feedUrl": "http://three.example/feed.xml",
         "artworkUrl600": "https://img.example/three600.jpg"},
        {"collectionId": 14, "feedUrl": "  "},
        {"collectionId": 15, "collectionName": "Show One Again", "feedUrl": "https://one.example/rss"},
        {"collectionId": 16, "collectionName": "Odd Art", "feedUrl": "https://odd.example/rss",
         "artworkUrl100": "javascript:alert(1)", "artworkUrl600": "http://img.example/odd600.jpg"}
    ]}"#;

    #[test]
    fn search_keeps_the_fields_the_ui_shows() {
        let shows = parse_search(SEARCH.as_bytes()).unwrap();
        assert_eq!(shows.len(), 3);
        assert_eq!(
            shows[0],
            Listing {
                feed_url: "https://one.example/rss".into(),
                title: "Show One".into(),
                author: "Ann".into(),
                artwork_url: Some("https://img.example/one100.jpg".into()),
                genre: "History".into(),
                episode_count: Some(120),
            }
        );
    }

    #[test]
    fn search_falls_back_on_missing_fields() {
        let shows = parse_search(SEARCH.as_bytes()).unwrap();
        assert_eq!(
            shows[1].artwork_url.as_deref(),
            Some("https://img.example/three600.jpg")
        );
        assert_eq!(shows[1].author, "");
        assert_eq!(shows[1].genre, "");
        assert_eq!(shows[1].episode_count, None);
        // The small image is not a web address, so the large one is used, over http as it is.
        assert_eq!(
            shows[2].artwork_url.as_deref(),
            Some("http://img.example/odd600.jpg")
        );
    }

    #[test]
    fn search_lists_a_feed_once() {
        let shows = parse_search(SEARCH.as_bytes()).unwrap();
        let titles: Vec<_> = shows.iter().map(|show| show.title.as_str()).collect();
        assert_eq!(titles, ["Show One", "Only Large Art", "Odd Art"]);
    }

    #[test]
    fn search_with_no_results_is_empty() {
        assert!(
            parse_search(br#"{"resultCount": 0, "results": []}"#)
                .unwrap()
                .is_empty()
        );
        assert!(parse_search(b"{}").unwrap().is_empty());
    }

    #[test]
    fn unreadable_replies_are_errors() {
        assert!(matches!(parse_search(b"<html>"), Err(Error::Directory(_))));
        assert!(matches!(parse_chart(b"{}"), Err(Error::Directory(_))));
    }

    #[test]
    fn lookup_follows_the_order_asked_for_and_drops_strangers() {
        let body = r#"{"resultCount": 4, "results": [
            {"collectionId": 3, "collectionName": "Third", "feedUrl": "https://c.example/"},
            {"collectionId": 1, "collectionName": "First", "feedUrl": "https://a.example/"},
            {"collectionId": 99, "collectionName": "Stranger", "feedUrl": "https://z.example/"},
            {"collectionId": 2, "collectionName": "No feed"}
        ]}"#;
        let titles: Vec<_> = parse_lookup(body.as_bytes(), &[1, 2, 3])
            .unwrap()
            .into_iter()
            .map(|show| show.title)
            .collect();
        assert_eq!(titles, ["First", "Third"]);
    }

    #[test]
    fn chart_ids_come_out_in_rank_order() {
        let chart = r#"{"feed": {"author": {"name": {"label": "iTunes Store"}}, "entry": [
            {"im:name": {"label": "A"}, "id": {"label": "https://x.example/a", "attributes": {"im:id": "1200361736"}}},
            {"im:name": {"label": "B"}, "id": {"label": "https://x.example/b", "attributes": {"im:id": "42"}}}
        ]}}"#;
        assert_eq!(parse_chart(chart.as_bytes()).unwrap(), [1200361736, 42]);
        assert!(parse_chart(br#"{"feed": {}}"#).unwrap().is_empty());
    }

    #[test]
    fn a_chart_of_one_show_is_an_object_and_still_read() {
        let chart = r#"{"feed": {"entry":
            {"im:name": {"label": "Only"}, "id": {"label": "https://x.example/a", "attributes": {"im:id": "7"}}}
        }}"#;
        assert_eq!(parse_chart(chart.as_bytes()).unwrap(), [7]);
    }

    #[test]
    fn chart_urls_name_the_country_and_category() {
        assert_eq!(
            chart_url("gb", Some(1318)),
            "https://itunes.apple.com/gb/rss/toppodcasts/limit=50/genre=1318/json"
        );
        assert_eq!(
            chart_url("us", None),
            "https://itunes.apple.com/us/rss/toppodcasts/limit=50/json"
        );
    }
}
