use super::*;

const FEED: &str = include_str!("../../tests/fixtures/feed.xml");
const DURATIONS: &str = include_str!("../../tests/fixtures/durations.xml");
const ATOM: &str = include_str!("../../tests/fixtures/atom.xml");
const NO_GUID: &str = include_str!("../../tests/fixtures/no_guid.xml");
const MALFORMED: &str = include_str!("../../tests/fixtures/malformed.xml");

fn episode<'a>(feed: &'a ParsedFeed, title: &str) -> &'a ParsedEpisode {
    feed.episodes
        .iter()
        .find(|episode| episode.title == title)
        .unwrap_or_else(|| panic!("no episode titled {title}"))
}

#[test]
fn duration_formats() {
    assert_eq!(parse_duration_ms("3723"), Some(3_723_000));
    assert_eq!(parse_duration_ms("62:03"), Some(3_723_000));
    assert_eq!(parse_duration_ms("1:02:03"), Some(3_723_000));
    assert_eq!(parse_duration_ms("1:02:03.5"), Some(3_723_500));
}

#[test]
fn duration_edge_cases() {
    assert_eq!(parse_duration_ms(" 01:02:03 "), Some(3_723_000));
    assert_eq!(parse_duration_ms("0:05"), Some(5_000));
    assert_eq!(parse_duration_ms("12.25"), Some(12_250));
    assert_eq!(parse_duration_ms("12.05"), Some(12_050));
    assert_eq!(parse_duration_ms("12.123456"), Some(12_123));
    assert_eq!(parse_duration_ms("0"), Some(0));
}

#[test]
fn duration_garbage_is_none() {
    for text in ["", ":", "1:2:3:4", "abc", "1:xx", "-5", "1.2.3", "1:02:"] {
        assert_eq!(parse_duration_ms(text), None, "{text:?}");
    }
    assert_eq!(parse_duration_ms("18446744073709551615"), None);
}

#[test]
fn rss_channel_details() {
    let feed = parse(FEED.as_bytes()).unwrap();
    assert_eq!(feed.title, "Test Cast");
    assert_eq!(feed.author, "Ada Host");
    assert_eq!(feed.description, "A show about testing.");
    assert_eq!(feed.link.as_deref(), Some("https://example.com/testcast"));
    assert_eq!(feed.artwork_url.as_deref(), Some("{{BASE}}/art/show.jpg"));
}

#[test]
fn rss_episodes_without_enclosures_are_skipped() {
    let feed = parse(FEED.as_bytes()).unwrap();
    let titles: Vec<_> = feed.episodes.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["Episode 3", "Episode 2", "Episode 1"]);
}

#[test]
fn rss_episode_details() {
    let feed = parse(FEED.as_bytes()).unwrap();
    let ep3 = episode(&feed, "Episode 3");
    assert_eq!(ep3.guid, "tc-003");
    assert_eq!(ep3.description, "<p>Third <b>episode</b>.</p>");
    assert_eq!(ep3.published, Some(1_737_360_000));
    assert_eq!(ep3.duration_ms, Some(3_723_000));
    assert_eq!(ep3.enclosure_url, "{{BASE}}/audio/ep3.mp3");
    assert_eq!(ep3.enclosure_type.as_deref(), Some("audio/mpeg"));
    assert_eq!(ep3.enclosure_length, Some(250_000));
    assert_eq!((ep3.season, ep3.episode), (Some(1), Some(3)));
    assert!(ep3.explicit);
    assert_eq!(ep3.artwork_url.as_deref(), Some("{{BASE}}/art/ep3.jpg"));

    let ep2 = episode(&feed, "Episode 2");
    assert_eq!(ep2.description, "Plain summary of episode two.");
    assert!(!ep2.explicit);
    assert_eq!(ep2.artwork_url, None);
}

#[test]
fn every_duration_spelling_in_a_feed() {
    let feed = parse(DURATIONS.as_bytes()).unwrap();
    let durations: Vec<_> = feed.episodes.iter().map(|e| e.duration_ms).collect();
    assert_eq!(
        durations,
        [
            Some(3_723_000),
            Some(3_723_000),
            Some(3_723_000),
            Some(3_723_500),
            None,
            None
        ]
    );
}

#[test]
fn atom_feed() {
    let feed = parse(ATOM.as_bytes()).unwrap();
    assert_eq!(feed.title, "Atom Cast");
    assert_eq!(feed.author, "Grace Host");
    assert_eq!(feed.description, "A podcast published as Atom.");
    assert_eq!(feed.link.as_deref(), Some("https://example.com/atomcast"));
    assert_eq!(feed.artwork_url.as_deref(), Some("{{BASE}}/art/atom.jpg"));

    let titles: Vec<_> = feed.episodes.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["Second Atom Episode", "First Atom Episode"]);

    let second = episode(&feed, "Second Atom Episode");
    assert_eq!(second.guid, "urn:uuid:5f1c0000-0000-4000-8000-000000000012");
    assert_eq!(second.description, "Summary of the second episode.");
    assert_eq!(second.published, Some(1_738_573_200));
    assert_eq!(second.enclosure_url, "{{BASE}}/audio/atom2.mp3");
    assert_eq!(second.enclosure_length, Some(120_000));

    let first = episode(&feed, "First Atom Episode");
    assert_eq!(first.description, "<p>Show notes.</p>");
    assert_eq!(first.published, Some(1_737_972_000));
}

#[test]
fn missing_guid_falls_back_to_the_enclosure_url() {
    let feed = parse(NO_GUID.as_bytes()).unwrap();
    assert_eq!(
        episode(&feed, "Alpha").guid,
        "https://example.com/alpha.mp3"
    );
}

#[test]
fn duplicate_guids_and_enclosures_keep_the_first_item() {
    let feed = parse(NO_GUID.as_bytes()).unwrap();
    let titles: Vec<_> = feed.episodes.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["Alpha", "Beta"]);
}

#[test]
fn malformed_documents_are_errors() {
    for (name, document) in [
        ("unclosed", MALFORMED),
        ("empty", ""),
        ("whitespace", "   \n"),
        (
            "html",
            "<!DOCTYPE html><html><body><p>Not a feed</p></body></html>",
        ),
        ("plain text", "this is not xml at all"),
        ("truncated", &FEED[..FEED.len() / 2]),
        ("binary", "\u{0}\u{1}\u{2}\u{fffd}\u{fffd}<"),
        ("json", "{\"version\": \"https://jsonfeed.org/version/1\"}"),
    ] {
        assert!(
            matches!(parse(document.as_bytes()), Err(Error::Parse(_))),
            "{name} should be rejected"
        );
    }
}

#[test]
fn invalid_utf8_is_an_error() {
    assert!(parse(&[0xff, 0xfe, 0xfd, b'<', b'r', b's', b's']).is_err());
}
