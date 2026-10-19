use antistreaming_podcasts::{Error, Podcasts};

use crate::common::{Env, fixture};

#[test]
fn etag_revalidation_yields_not_modified() {
    let mut env = Env::new();
    env.server
        .serve_feed_with_etag("/feed.xml", "feed.xml", "\"v1\"");
    let feed = env
        .podcasts
        .subscribe(&env.server.url("/feed.xml"))
        .unwrap();

    let summary = env.podcasts.refresh(feed.id).unwrap();
    assert!(summary.not_modified);
    assert_eq!((summary.new_episodes, summary.updated_episodes), (0, 0));

    let requests = env.server.requests_for("/feed.xml");
    assert_eq!(requests[0].if_none_match, None);
    assert_eq!(requests[1].if_none_match.as_deref(), Some("\"v1\""));
    assert_eq!(env.podcasts.episodes(feed.id).unwrap().len(), 3);
}

#[test]
fn last_modified_revalidation_yields_not_modified() {
    let mut env = Env::new();
    let stamp = "Mon, 20 Jan 2025 09:00:00 GMT";
    env.server
        .serve_feed_with_last_modified("/feed.xml", "feed.xml", stamp);
    let feed = env
        .podcasts
        .subscribe(&env.server.url("/feed.xml"))
        .unwrap();

    assert!(env.podcasts.refresh(feed.id).unwrap().not_modified);
    let requests = env.server.requests_for("/feed.xml");
    assert_eq!(requests[1].if_modified_since.as_deref(), Some(stamp));
}

#[test]
fn not_modified_still_records_the_check() {
    let mut env = Env::new();
    env.server
        .serve_feed_with_etag("/feed.xml", "feed.xml", "\"v1\"");
    let feed = env
        .podcasts
        .subscribe(&env.server.url("/feed.xml"))
        .unwrap();
    let before = feed.last_refreshed.unwrap();

    env.podcasts.refresh(feed.id).unwrap();
    assert!(env.podcasts.feeds().unwrap()[0].last_refreshed.unwrap() >= before);
}

#[test]
fn refresh_adds_only_the_new_episodes_and_renames_in_place() {
    let mut env = Env::new();
    env.server
        .serve_feed_with_etag("/feed.xml", "feed.xml", "\"v1\"");
    let feed = env
        .podcasts
        .subscribe(&env.server.url("/feed.xml"))
        .unwrap();
    let before = env.podcasts.episodes(feed.id).unwrap();

    env.server
        .serve_feed_with_etag("/feed.xml", "feed_updated.xml", "\"v2\"");
    let summary = env.podcasts.refresh(feed.id).unwrap();
    assert!(!summary.not_modified);
    assert_eq!((summary.new_episodes, summary.updated_episodes), (1, 1));

    let after = env.podcasts.episodes(feed.id).unwrap();
    let titles: Vec<_> = after.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Episode 4",
            "Episode 3 (remastered)",
            "Episode 2",
            "Episode 1"
        ]
    );

    // The renamed episode keeps its row, so nothing that references it breaks.
    let renamed = after.iter().find(|e| e.guid == "tc-003").unwrap();
    let original = before.iter().find(|e| e.guid == "tc-003").unwrap();
    assert_eq!(renamed.id, original.id);
    assert_eq!(env.podcasts.feeds().unwrap()[0].unplayed_count, 4);

    // Validators were replaced, so the next check is a cheap 304.
    assert!(env.podcasts.refresh(feed.id).unwrap().not_modified);
    let last = env.server.requests_for("/feed.xml").pop().unwrap();
    assert_eq!(last.if_none_match.as_deref(), Some("\"v2\""));
}

#[test]
fn refreshing_an_unchanged_feed_without_validators_changes_nothing() {
    let (mut env, feed) = Env::subscribed();
    let before = env.podcasts.episodes(feed.id).unwrap();

    let summary = env.podcasts.refresh(feed.id).unwrap();
    assert!(!summary.not_modified);
    assert_eq!((summary.new_episodes, summary.updated_episodes), (0, 0));
    assert_eq!(env.podcasts.episodes(feed.id).unwrap(), before);
}

#[test]
fn refresh_keeps_listening_state_and_downloads() {
    let (mut env, feed) = Env::subscribed();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let (ep3, ep2) = (episodes[0].id, episodes[1].id);
    let file = env.download(ep3);
    env.podcasts.set_position(ep3, 90_000).unwrap();
    env.podcasts.set_played(ep2, true).unwrap();

    env.server.serve_fixture("/feed.xml", "feed_updated.xml");
    env.podcasts.refresh(feed.id).unwrap();

    let after = env.podcasts.episodes(feed.id).unwrap();
    let renamed = after.iter().find(|e| e.id == ep3).unwrap();
    assert_eq!(renamed.title, "Episode 3 (remastered)");
    assert_eq!(renamed.position_ms, 90_000);
    assert_eq!(renamed.file.as_ref(), Some(&file));
    assert!(after.iter().find(|e| e.id == ep2).unwrap().played);
}

#[test]
fn feed_level_details_are_updated() {
    let (mut env, feed) = Env::subscribed();
    let xml = fixture("feed.xml")
        .replace(
            "<title>Test Cast</title>",
            "<title>Test Cast Reloaded</title>",
        )
        .replace("Ada Host", "Ada and Friends");
    env.server.serve_feed("/feed.xml", &xml);
    env.podcasts.refresh(feed.id).unwrap();

    let updated = &env.podcasts.feeds().unwrap()[0];
    assert_eq!(updated.title, "Test Cast Reloaded");
    assert_eq!(updated.author, "Ada and Friends");
}

#[test]
fn regenerated_guids_do_not_duplicate_episodes() {
    let (mut env, feed) = Env::subscribed();
    let before = env.podcasts.episodes(feed.id).unwrap();
    let xml = fixture("feed.xml").replace("tc-00", "migrated-00");
    env.server.serve_feed("/feed.xml", &xml);

    let summary = env.podcasts.refresh(feed.id).unwrap();
    assert_eq!((summary.new_episodes, summary.updated_episodes), (0, 3));
    let after = env.podcasts.episodes(feed.id).unwrap();
    assert_eq!(after.len(), 3);
    assert_eq!(
        after.iter().map(|e| e.id).collect::<Vec<_>>(),
        before.iter().map(|e| e.id).collect::<Vec<_>>()
    );
}

#[test]
fn a_broken_feed_leaves_stored_data_alone() {
    let (mut env, feed) = Env::subscribed();
    let before = env.podcasts.episodes(feed.id).unwrap();

    env.server.serve_fixture("/feed.xml", "malformed.xml");
    assert!(matches!(
        env.podcasts.refresh(feed.id),
        Err(Error::Parse(_))
    ));
    env.server.fail("/feed.xml", 503);
    assert!(matches!(
        env.podcasts.refresh(feed.id),
        Err(Error::Status { status: 503, .. })
    ));

    assert_eq!(env.podcasts.episodes(feed.id).unwrap(), before);
}

#[test]
fn refreshing_an_unknown_feed_is_an_error() {
    let mut env = Env::new();
    assert!(matches!(
        env.podcasts.refresh(42),
        Err(Error::FeedNotFound(42))
    ));
}

#[test]
fn refresh_all_reports_each_feed_separately() {
    let (mut env, first) = Env::subscribed();
    env.server.serve_fixture("/atom.xml", "atom.xml");
    let second = env
        .podcasts
        .subscribe(&env.server.url("/atom.xml"))
        .unwrap();

    env.server.fail("/atom.xml", 500);
    env.server.serve_fixture("/feed.xml", "feed_updated.xml");
    let results = env.podcasts.refresh_all().unwrap();

    assert_eq!(results.len(), 2);
    let outcome = |id: i64| {
        &results
            .iter()
            .find(|(feed_id, _)| *feed_id == id)
            .unwrap()
            .1
    };
    assert_eq!(outcome(first.id).as_ref().unwrap().new_episodes, 1);
    let error = outcome(second.id).as_ref().unwrap_err();
    assert!(error.contains("500"), "{error}");
}

#[test]
fn two_handles_can_share_one_database() {
    let (env, feed) = Env::subscribed();
    let second = Podcasts::open(&env.dir.path().join("podcasts.db"), &env.download_dir()).unwrap();
    assert_eq!(second.episodes(feed.id).unwrap().len(), 3);
}
