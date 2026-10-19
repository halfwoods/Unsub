use antistreaming_podcasts::{Error, Podcasts};

use crate::common::{Env, fixture};

#[test]
fn export_then_import_round_trips_the_subscriptions() {
    let (mut env, _) = Env::subscribed();
    env.server.serve_fixture("/atom.xml", "atom.xml");
    env.podcasts
        .subscribe(&env.server.url("/atom.xml"))
        .unwrap();
    let exported = env.podcasts.export_opml().unwrap();

    assert!(exported.starts_with("<?xml"));
    assert!(exported.contains(&format!("xmlUrl=\"{}\"", env.server.url("/feed.xml"))));
    assert!(exported.contains("htmlUrl=\"https://example.com/testcast\""));

    let other = tempfile::tempdir().unwrap();
    let mut fresh =
        Podcasts::open(&other.path().join("db"), &other.path().join("downloads")).unwrap();
    let imported = fresh.import_opml(&exported).unwrap();

    assert_eq!(imported.len(), 2);
    assert!(imported.iter().all(Result::is_ok));
    let titles = |podcasts: &Podcasts| -> Vec<(String, String)> {
        podcasts
            .feeds()
            .unwrap()
            .into_iter()
            .map(|f| (f.title, f.url))
            .collect()
    };
    assert_eq!(titles(&fresh), titles(&env.podcasts));
    let feed = &fresh.feeds().unwrap()[0];
    assert_eq!(fresh.episodes(feed.id).unwrap().len(), 2);
}

#[test]
fn importing_the_same_opml_twice_does_not_duplicate_feeds() {
    let (mut env, _) = Env::subscribed();
    let exported = env.podcasts.export_opml().unwrap();

    let results = env.podcasts.import_opml(&exported).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(env.podcasts.feeds().unwrap().len(), 1);
}

#[test]
fn import_reports_each_outline_and_skips_folders() {
    let mut env = Env::new();
    env.server.serve_fixture("/feed.xml", "feed.xml");
    env.server.serve_fixture("/atom.xml", "atom.xml");
    let opml = fixture("subscriptions.opml").replace("{{BASE}}", &env.server.base);

    let results = env.podcasts.import_opml(&opml).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].as_ref().unwrap().title, "Test Cast");
    assert_eq!(results[1].as_ref().unwrap().title, "Atom Cast");
    let failure = results[2].as_ref().unwrap_err();
    assert!(failure.contains("404"), "{failure}");
    assert_eq!(env.podcasts.feeds().unwrap().len(), 2);
}

#[test]
fn export_escapes_markup_in_titles_and_urls() {
    let mut env = Env::new();
    let xml = fixture("feed.xml").replace(
        "<title>Test Cast</title>",
        "<title>Fish &amp; \"Chips\" &lt;3</title>",
    );
    env.server.serve_feed("/feed.xml?a=1&b=2", &xml);
    env.podcasts
        .subscribe(&env.server.url("/feed.xml?a=1&b=2"))
        .unwrap();

    let exported = env.podcasts.export_opml().unwrap();
    assert!(
        exported.contains("text=\"Fish &amp; &quot;Chips&quot; &lt;3\""),
        "{exported}"
    );
    assert!(exported.contains("feed.xml?a=1&amp;b=2"), "{exported}");

    let results = env.podcasts.import_opml(&exported).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].is_ok(), "{results:?}");
    assert_eq!(env.podcasts.feeds().unwrap().len(), 1);
}

#[test]
fn an_empty_library_exports_a_valid_empty_document() {
    let mut env = Env::new();
    let exported = env.podcasts.export_opml().unwrap();
    assert!(exported.contains("<opml"));
    assert!(env.podcasts.import_opml(&exported).unwrap().is_empty());
}

#[test]
fn malformed_opml_is_an_error_and_imports_nothing() {
    let (mut env, _) = Env::subscribed();
    let good = format!("<outline xmlUrl=\"{}\"/>", env.server.url("/atom.xml"));
    env.server.serve_fixture("/atom.xml", "atom.xml");

    for (name, document) in [
        ("empty", String::new()),
        ("not opml", "<html><body></body></html>".to_owned()),
        ("garbage", "definitely not xml".to_owned()),
        (
            "mismatched tags",
            format!("<opml><body>{good}</head></opml>"),
        ),
        ("truncated", format!("<opml><body>{good}")),
        (
            "broken attribute",
            "<opml><body><outline xmlUrl=\"oops></body></opml>".to_owned(),
        ),
    ] {
        assert!(
            matches!(env.podcasts.import_opml(&document), Err(Error::Opml(_))),
            "{name}"
        );
    }
    assert_eq!(
        env.podcasts.feeds().unwrap().len(),
        1,
        "nothing was subscribed along the way"
    );
}
