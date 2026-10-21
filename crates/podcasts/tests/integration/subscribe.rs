use unsub_podcasts::{Error, FeedSettings, KeepRule, Podcasts, Subscribe};

use crate::common::{Env, fixture};

#[test]
fn subscribe_stores_the_feed_and_its_episodes() {
    let (env, feed) = Env::subscribed();

    assert_eq!(feed.url, env.server.url("/feed.xml"));
    assert_eq!(feed.title, "Test Cast");
    assert_eq!(feed.author, "Ada Host");
    assert_eq!(feed.description, "A show about testing.");
    assert_eq!(feed.artwork_url, Some(env.server.url("/art/show.jpg")));
    assert_eq!(feed.link.as_deref(), Some("https://example.com/testcast"));
    assert_eq!(feed.unplayed_count, 3);
    assert!(feed.last_refreshed.is_some());
    assert_eq!(feed.settings.auto_download_latest, 1);
    assert_eq!(feed.settings.keep, KeepRule::All);

    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let titles: Vec<_> = episodes.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["Episode 3", "Episode 2", "Episode 1"]);

    let newest = &episodes[0];
    assert_eq!(newest.feed_id, feed.id);
    assert_eq!(newest.guid, "tc-003");
    assert_eq!(newest.published, Some(1_737_360_000));
    assert_eq!(newest.duration_ms, Some(3_723_000));
    assert_eq!(newest.enclosure_url, env.server.url("/audio/ep3.mp3"));
    assert_eq!(newest.enclosure_type.as_deref(), Some("audio/mpeg"));
    assert_eq!(newest.enclosure_length, Some(250_000));
    assert_eq!((newest.season, newest.episode), (Some(1), Some(3)));
    assert!(newest.explicit);
    assert!(!newest.played);
    assert_eq!(newest.position_ms, 0);
    assert_eq!(newest.file, None);
}

#[test]
fn a_new_feed_downloads_its_newest_episode_while_a_stored_one_keeps_its_own_setting() {
    let (mut env, first) = Env::subscribed();
    env.podcasts
        .set_settings(
            first.id,
            FeedSettings {
                auto_download_latest: 0,
                keep: KeepRule::All,
            },
        )
        .unwrap();

    env.server.serve_fixture("/atom.xml", "atom.xml");
    let second = env
        .podcasts
        .subscribe(&env.server.url("/atom.xml"))
        .unwrap();
    assert_eq!(second.settings.auto_download_latest, 1);

    // Subscribing to a stored feed again refreshes it, and reopening the database changes nothing.
    env.podcasts
        .subscribe(&env.server.url("/feed.xml"))
        .unwrap();
    let reopened =
        Podcasts::open(&env.dir.path().join("podcasts.db"), &env.download_dir()).unwrap();
    let mut settings: Vec<_> = reopened
        .feeds()
        .unwrap()
        .into_iter()
        .map(|feed| (feed.id, feed.settings.auto_download_latest))
        .collect();
    settings.sort();
    assert_eq!(settings, [(first.id, 0), (second.id, 1)]);
}

#[test]
fn feeds_imported_in_bulk_start_with_nothing_downloading_but_a_single_subscribe_still_does() {
    let mut env = Env::new();
    env.server.serve_fixture("/feed.xml", "feed.xml");
    env.server.serve_fixture("/atom.xml", "atom.xml");
    env.server.serve_fixture("/noguid.xml", "no_guid.xml");
    env.server.serve_fixture("/durations.xml", "durations.xml");

    let opml = fixture("subscriptions.opml").replace("{{BASE}}", &env.server.base);
    let imported = env.podcasts.import_opml(&opml).unwrap();
    let fetched = Podcasts::fetch(&env.server.url("/noguid.xml")).unwrap();
    let bulk = env.podcasts.save_fetched(fetched, Subscribe::Bulk).unwrap();

    assert_eq!(imported.iter().flatten().count(), 2);
    for feed in imported.iter().flatten().chain([&bulk]) {
        assert_eq!(feed.settings.auto_download_latest, 0, "{}", feed.title);
    }
    assert!(env.podcasts.pending_auto_downloads().unwrap().is_empty());

    let single = env
        .podcasts
        .subscribe(&env.server.url("/durations.xml"))
        .unwrap();
    assert_eq!(single.settings.auto_download_latest, 1);
    assert_eq!(env.podcasts.pending_auto_downloads().unwrap().len(), 1);
}

#[test]
fn requests_identify_the_app() {
    let (env, _) = Env::subscribed();
    let requests = env.server.requests_for("/feed.xml");
    let agent = requests[0].user_agent.as_deref().unwrap();
    assert!(agent.starts_with("Unsub/"), "{agent}");
}

#[test]
fn every_duration_format_survives_the_round_trip() {
    let mut env = Env::new();
    env.server.serve_fixture("/durations.xml", "durations.xml");
    let feed = env
        .podcasts
        .subscribe(&env.server.url("/durations.xml"))
        .unwrap();

    // The feed carries no dates, so episodes come back in document order.
    let durations: Vec<_> = env
        .podcasts
        .episodes(feed.id)
        .unwrap()
        .into_iter()
        .map(|e| (e.guid, e.duration_ms))
        .collect();
    let expected = [
        ("d-seconds", Some(3_723_000)),
        ("d-mmss", Some(3_723_000)),
        ("d-hhmmss", Some(3_723_000)),
        ("d-frac", Some(3_723_500)),
        ("d-junk", None),
        ("d-none", None),
    ];
    let expected: Vec<_> = expected
        .iter()
        .map(|(guid, ms)| (guid.to_string(), *ms))
        .collect();
    assert_eq!(durations, expected);
}

#[test]
fn atom_feeds_are_supported() {
    let mut env = Env::new();
    env.server.serve_fixture("/atom.xml", "atom.xml");
    let feed = env
        .podcasts
        .subscribe(&env.server.url("/atom.xml"))
        .unwrap();

    assert_eq!(feed.title, "Atom Cast");
    assert_eq!(feed.author, "Grace Host");
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let titles: Vec<_> = episodes.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["Second Atom Episode", "First Atom Episode"]);
    assert_eq!(
        episodes[0].enclosure_url,
        env.server.url("/audio/atom2.mp3")
    );
    assert_eq!(episodes[0].enclosure_length, Some(120_000));
}

#[test]
fn episodes_without_guid_or_with_repeated_ones_are_deduplicated() {
    let mut env = Env::new();
    env.server.serve_fixture("/noguid.xml", "no_guid.xml");
    let feed = env
        .podcasts
        .subscribe(&env.server.url("/noguid.xml"))
        .unwrap();

    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let titles: Vec<_> = episodes.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["Alpha", "Beta"]);
    assert_eq!(episodes[0].guid, "https://example.com/alpha.mp3");

    let summary = env.podcasts.refresh(feed.id).unwrap();
    assert_eq!((summary.new_episodes, summary.updated_episodes), (0, 0));
    assert_eq!(env.podcasts.episodes(feed.id).unwrap().len(), 2);
}

#[test]
fn redirects_are_followed_and_the_final_url_is_stored() {
    let mut env = Env::new();
    env.server.serve_fixture("/feed.xml", "feed.xml");
    env.server.redirect("/old/feed.xml", "/feed.xml");

    let feed = env
        .podcasts
        .subscribe(&env.server.url("/old/feed.xml"))
        .unwrap();
    assert_eq!(feed.url, env.server.url("/feed.xml"));

    // Refreshing goes straight to the new address.
    env.podcasts.refresh(feed.id).unwrap();
    assert_eq!(env.server.requests_for("/old/feed.xml").len(), 1);
}

#[test]
fn the_address_a_redirected_feed_was_asked_for_is_remembered_once() {
    let mut env = Env::new();
    env.server.serve_fixture("/feed.xml", "feed.xml");
    env.server.redirect("/old/feed.xml", "/feed.xml");
    let asked = env.server.url("/old/feed.xml");

    let feed = env.podcasts.subscribe(&asked).unwrap();
    let again = env.podcasts.subscribe(&asked).unwrap();

    assert_eq!(feed.aliases, std::slice::from_ref(&asked));
    assert_eq!(again.id, feed.id);
    assert_eq!(again.aliases, [asked]);
    // Feeds that did not redirect have nothing to remember.
    let (_, plain) = Env::subscribed();
    assert!(plain.aliases.is_empty());
}

#[test]
fn a_fetched_feed_can_be_stored_later_by_another_handle() {
    let mut env = Env::new();
    env.server.serve_fixture("/feed.xml", "feed.xml");

    let fetched = Podcasts::fetch(&env.server.url("/feed.xml")).unwrap();
    let feed = env.podcasts.save_fetched(fetched, Subscribe::One).unwrap();

    assert_eq!(feed.title, "Test Cast");
    assert_eq!(env.podcasts.feeds().unwrap(), [feed]);
}

#[test]
fn itpc_and_podcast_schemes_are_normalized() {
    let mut env = Env::new();
    env.server.serve_fixture("/feed.xml", "feed.xml");
    let host = env.server.base.strip_prefix("http://").unwrap().to_owned();

    let feed = env
        .podcasts
        .subscribe(&format!("itpc://{host}/feed.xml"))
        .unwrap();
    assert_eq!(feed.url, env.server.url("/feed.xml"));

    let again = env
        .podcasts
        .subscribe(&format!("podcast://{host}/feed.xml"))
        .unwrap();
    assert_eq!(again.id, feed.id);
}

#[test]
fn subscribing_twice_does_not_duplicate_anything() {
    let (mut env, feed) = Env::subscribed();
    let again = env
        .podcasts
        .subscribe(&env.server.url("/feed.xml"))
        .unwrap();

    assert_eq!(again.id, feed.id);
    assert_eq!(env.podcasts.feeds().unwrap().len(), 1);
    assert_eq!(env.podcasts.episodes(feed.id).unwrap().len(), 3);
}

#[test]
fn malformed_xml_is_an_error_and_stores_nothing() {
    let mut env = Env::new();
    env.server.serve_fixture("/broken.xml", "malformed.xml");
    env.server.serve_feed(
        "/page.html",
        "<!DOCTYPE html><html><body>hello</body></html>",
    );
    env.server.serve_feed("/empty.xml", "");

    for path in ["/broken.xml", "/page.html", "/empty.xml"] {
        let result = env.podcasts.subscribe(&env.server.url(path));
        assert!(matches!(result, Err(Error::Parse(_))), "{path}: {result:?}");
    }
    assert!(env.podcasts.feeds().unwrap().is_empty());
}

#[test]
fn http_errors_and_bad_urls_are_errors() {
    let mut env = Env::new();
    env.server.fail("/boom.xml", 500);

    assert!(matches!(
        env.podcasts.subscribe(&env.server.url("/missing.xml")),
        Err(Error::Status { status: 404, .. })
    ));
    assert!(matches!(
        env.podcasts.subscribe(&env.server.url("/boom.xml")),
        Err(Error::Status { status: 500, .. })
    ));
    assert!(matches!(
        env.podcasts.subscribe("ftp://example.com/feed"),
        Err(Error::InvalidUrl(_))
    ));
    assert!(matches!(
        env.podcasts.subscribe("not a url"),
        Err(Error::InvalidUrl(_))
    ));
    assert!(env.podcasts.feeds().unwrap().is_empty());
}

#[test]
fn feeds_are_listed_by_title_ignoring_case() {
    let mut env = Env::new();
    env.server.serve_fixture("/feed.xml", "feed.xml");
    env.server.serve_fixture("/atom.xml", "atom.xml");
    env.server.serve_feed(
        "/lower.xml",
        &fixture("feed.xml").replace("<title>Test Cast</title>", "<title>aardvark</title>"),
    );

    for path in ["/feed.xml", "/atom.xml", "/lower.xml"] {
        env.podcasts.subscribe(&env.server.url(path)).unwrap();
    }
    let titles: Vec<_> = env
        .podcasts
        .feeds()
        .unwrap()
        .into_iter()
        .map(|f| f.title)
        .collect();
    assert_eq!(titles, ["aardvark", "Atom Cast", "Test Cast"]);
}

#[test]
fn unsubscribe_removes_the_feed_and_its_episodes() {
    let (mut env, feed) = Env::subscribed();
    env.podcasts.unsubscribe(feed.id, false).unwrap();

    assert!(env.podcasts.feeds().unwrap().is_empty());
    assert!(env.podcasts.episodes(feed.id).unwrap().is_empty());
    assert!(matches!(
        env.podcasts.unsubscribe(feed.id, false),
        Err(Error::FeedNotFound(_))
    ));
}

#[test]
fn unsubscribe_can_keep_or_delete_downloaded_files() {
    let (mut env, feed) = Env::subscribed();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let kept = env.download(episodes[0].id);
    env.podcasts.unsubscribe(feed.id, false).unwrap();
    assert!(kept.exists(), "files stay when delete_files is false");

    let feed = env
        .podcasts
        .subscribe(&env.server.url("/feed.xml"))
        .unwrap();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let file = env.download(episodes[1].id);
    let folder = file.parent().unwrap().to_owned();
    std::fs::write(folder.join(format!("{}.part", episodes[2].id)), b"half").unwrap();
    env.podcasts.unsubscribe(feed.id, true).unwrap();
    assert!(!file.exists());
    assert!(!folder.join(format!("{}.part", episodes[2].id)).exists());
    assert!(kept.exists(), "unrelated files are untouched");
}

#[test]
fn data_survives_reopening_the_database() {
    let (env, feed) = Env::subscribed();
    let Env {
        dir,
        server,
        podcasts,
    } = env;
    drop(podcasts);

    let reopened = Podcasts::open(
        &dir.path().join("podcasts.db"),
        &dir.path().join("downloads"),
    )
    .unwrap();
    assert_eq!(reopened.feeds().unwrap(), vec![feed.clone()]);
    assert_eq!(reopened.episodes(feed.id).unwrap().len(), 3);
    drop(server);
}

#[test]
fn a_database_from_a_newer_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("podcasts.db");
    drop(Podcasts::open(&db, &dir.path().join("downloads")).unwrap());

    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.pragma_update(None, "user_version", 99).unwrap();
    drop(conn);

    assert!(matches!(
        Podcasts::open(&db, &dir.path().join("downloads")),
        Err(Error::UnsupportedSchema(99))
    ));
}

#[test]
fn public_types_serialize_with_camel_case_keys() {
    let (env, feed) = Env::subscribed();
    let feed_json = serde_json::to_value(&feed).unwrap();
    for key in ["artworkUrl", "lastRefreshed", "unplayedCount", "settings"] {
        assert!(feed_json.get(key).is_some(), "feed is missing {key}");
    }
    assert_eq!(feed_json["settings"]["autoDownloadLatest"], 1);
    assert_eq!(feed_json["settings"]["keep"], "all");

    let episode = &env.podcasts.episodes(feed.id).unwrap()[0];
    let episode_json = serde_json::to_value(episode).unwrap();
    for key in [
        "feedId",
        "durationMs",
        "enclosureUrl",
        "enclosureType",
        "enclosureLength",
        "positionMs",
        "artworkUrl",
    ] {
        assert!(episode_json.get(key).is_some(), "episode is missing {key}");
    }
    assert_eq!(
        serde_json::to_value(KeepRule::LastN(5)).unwrap(),
        serde_json::json!({ "lastN": 5 })
    );
    assert_eq!(
        serde_json::to_value(KeepRule::Unplayed).unwrap(),
        "unplayed"
    );
}

#[test]
fn the_handle_can_move_between_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<Podcasts>();
}
