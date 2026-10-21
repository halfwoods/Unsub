use unsub_podcasts::{Error, FeedSettings, KeepRule};

use crate::common::Env;

fn settings(auto_download_latest: u32, keep: KeepRule) -> FeedSettings {
    FeedSettings {
        auto_download_latest,
        keep,
    }
}

#[test]
fn a_new_subscription_wants_its_newest_episode_until_the_rule_is_turned_off() {
    let (mut env, feed) = Env::subscribed();
    let newest = env.podcasts.episodes(feed.id).unwrap()[0].id;
    assert_eq!(env.podcasts.pending_auto_downloads().unwrap(), vec![newest]);

    env.podcasts
        .set_settings(feed.id, settings(0, KeepRule::All))
        .unwrap();
    assert!(env.podcasts.pending_auto_downloads().unwrap().is_empty());
}

#[test]
fn auto_download_requests_the_newest_n_episodes() {
    let (mut env, feed) = Env::subscribed();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    env.podcasts
        .set_settings(feed.id, settings(2, KeepRule::All))
        .unwrap();

    assert_eq!(
        env.podcasts.pending_auto_downloads().unwrap(),
        vec![episodes[0].id, episodes[1].id]
    );

    env.download(episodes[0].id);
    assert_eq!(
        env.podcasts.pending_auto_downloads().unwrap(),
        vec![episodes[1].id]
    );

    // A listened episode inside the window is not worth fetching.
    env.podcasts.set_played(episodes[1].id, true).unwrap();
    assert!(env.podcasts.pending_auto_downloads().unwrap().is_empty());
}

#[test]
fn new_episodes_push_older_ones_out_of_the_window() {
    let (mut env, feed) = Env::subscribed();
    env.podcasts
        .set_settings(feed.id, settings(1, KeepRule::All))
        .unwrap();
    let newest = env.podcasts.episodes(feed.id).unwrap()[0].id;
    assert_eq!(env.podcasts.pending_auto_downloads().unwrap(), vec![newest]);

    env.server.serve_fixture("/feed.xml", "feed_updated.xml");
    env.podcasts.refresh(feed.id).unwrap();
    let pending = env.podcasts.pending_auto_downloads().unwrap();
    assert_eq!(pending.len(), 1);
    assert_ne!(pending[0], newest);
    assert_eq!(env.podcasts.episodes(feed.id).unwrap()[0].id, pending[0]);
}

#[test]
fn auto_download_rules_are_per_feed() {
    let (mut env, first) = Env::subscribed();
    env.podcasts
        .set_settings(first.id, settings(0, KeepRule::All))
        .unwrap();
    env.server.serve_fixture("/atom.xml", "atom.xml");
    let second = env
        .podcasts
        .subscribe(&env.server.url("/atom.xml"))
        .unwrap();
    env.podcasts
        .set_settings(second.id, settings(1, KeepRule::All))
        .unwrap();

    let pending = env.podcasts.pending_auto_downloads().unwrap();
    let atom_newest = env.podcasts.episodes(second.id).unwrap()[0].id;
    assert_eq!(pending, vec![atom_newest]);
    assert!(
        env.podcasts
            .episodes(first.id)
            .unwrap()
            .iter()
            .all(|e| !pending.contains(&e.id))
    );
}

#[test]
fn settings_are_stored_per_feed() {
    let (mut env, feed) = Env::subscribed();
    for keep in [KeepRule::LastN(7), KeepRule::Unplayed, KeepRule::All] {
        let wanted = settings(3, keep);
        env.podcasts.set_settings(feed.id, wanted.clone()).unwrap();
        assert_eq!(env.podcasts.feeds().unwrap()[0].settings, wanted);
    }
    assert!(matches!(
        env.podcasts.set_settings(99, FeedSettings::default()),
        Err(Error::FeedNotFound(99))
    ));
}

#[test]
fn keep_last_n_deletes_older_downloads() {
    let (mut env, feed) = Env::subscribed();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let files: Vec<_> = episodes.iter().map(|e| env.download(e.id)).collect();
    env.podcasts
        .set_settings(feed.id, settings(0, KeepRule::LastN(1)))
        .unwrap();

    let mut deleted = env.podcasts.apply_cleanup().unwrap();
    deleted.sort();
    let mut expected = vec![files[1].clone(), files[2].clone()];
    expected.sort();
    assert_eq!(deleted, expected);

    assert!(files[0].exists());
    assert!(!files[1].exists() && !files[2].exists());
    let after = env.podcasts.episodes(feed.id).unwrap();
    assert_eq!(after[0].file.as_ref(), Some(&files[0]));
    assert_eq!((after[1].file.clone(), after[2].file.clone()), (None, None));
    assert!(
        env.podcasts.apply_cleanup().unwrap().is_empty(),
        "a second pass has nothing left to do"
    );
}

#[test]
fn keep_unplayed_deletes_played_downloads() {
    let (mut env, feed) = Env::subscribed();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let files: Vec<_> = episodes.iter().map(|e| env.download(e.id)).collect();
    env.podcasts.set_played(episodes[1].id, true).unwrap();
    env.podcasts
        .set_settings(feed.id, settings(0, KeepRule::Unplayed))
        .unwrap();

    assert_eq!(
        env.podcasts.apply_cleanup().unwrap(),
        vec![files[1].clone()]
    );
    assert!(files[0].exists() && files[2].exists() && !files[1].exists());
}

#[test]
fn keep_all_deletes_nothing() {
    let (mut env, feed) = Env::subscribed();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let files: Vec<_> = episodes.iter().map(|e| env.download(e.id)).collect();
    env.podcasts.set_played(episodes[0].id, true).unwrap();

    assert!(env.podcasts.apply_cleanup().unwrap().is_empty());
    assert!(files.iter().all(|file| file.exists()));
}

#[test]
fn cleanup_ignores_episodes_that_were_never_downloaded() {
    let (mut env, feed) = Env::subscribed();
    env.podcasts
        .set_settings(feed.id, settings(0, KeepRule::LastN(1)))
        .unwrap();
    assert!(env.podcasts.apply_cleanup().unwrap().is_empty());
}

#[test]
fn cleanup_tolerates_files_that_are_already_gone() {
    let (mut env, feed) = Env::subscribed();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let old = env.download(episodes[2].id);
    std::fs::remove_file(&old).unwrap();
    env.podcasts
        .set_settings(feed.id, settings(0, KeepRule::LastN(1)))
        .unwrap();

    assert!(env.podcasts.apply_cleanup().unwrap().is_empty());
    assert_eq!(env.podcasts.episodes(feed.id).unwrap()[2].file, None);
}

#[test]
fn cleaned_episodes_are_not_downloaded_again_automatically() {
    let (mut env, feed) = Env::subscribed();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    for episode in &episodes {
        env.download(episode.id);
    }
    // Auto-download wants three, cleanup keeps one: without a memory they would fight forever.
    env.podcasts
        .set_settings(feed.id, settings(3, KeepRule::LastN(1)))
        .unwrap();
    env.podcasts.apply_cleanup().unwrap();

    assert!(env.podcasts.pending_auto_downloads().unwrap().is_empty());
}
