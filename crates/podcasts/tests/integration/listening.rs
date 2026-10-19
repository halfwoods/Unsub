use antistreaming_podcasts::Error;

use crate::common::Env;

#[test]
fn position_is_saved_per_episode() {
    let (mut env, feed) = Env::subscribed();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    env.podcasts.set_position(episodes[0].id, 754_321).unwrap();

    let after = env.podcasts.episodes(feed.id).unwrap();
    assert_eq!(after[0].position_ms, 754_321);
    assert_eq!(after[1].position_ms, 0);
    assert!(
        !after[0].played,
        "moving the playhead does not mark an episode played"
    );
}

#[test]
fn played_state_drives_the_unplayed_count() {
    let (mut env, feed) = Env::subscribed();
    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let unplayed = |env: &Env| env.podcasts.feeds().unwrap()[0].unplayed_count;
    assert_eq!(unplayed(&env), 3);

    env.podcasts.set_played(episodes[0].id, true).unwrap();
    env.podcasts.set_played(episodes[1].id, true).unwrap();
    assert_eq!(unplayed(&env), 1);
    assert!(env.podcasts.episodes(feed.id).unwrap()[0].played);

    env.podcasts.set_played(episodes[0].id, false).unwrap();
    assert_eq!(unplayed(&env), 2);
    assert!(!env.podcasts.episodes(feed.id).unwrap()[0].played);
}

#[test]
fn finishing_an_episode_rewinds_its_resume_point() {
    let (mut env, feed) = Env::subscribed();
    let episode = env.podcasts.episodes(feed.id).unwrap()[0].id;
    env.podcasts.set_position(episode, 3_000_000).unwrap();

    env.podcasts.set_played(episode, true).unwrap();
    assert_eq!(env.podcasts.episodes(feed.id).unwrap()[0].position_ms, 0);

    env.podcasts.set_position(episode, 5_000).unwrap();
    env.podcasts.set_played(episode, false).unwrap();
    assert_eq!(
        env.podcasts.episodes(feed.id).unwrap()[0].position_ms,
        5_000,
        "unmarking keeps the position"
    );
}

#[test]
fn unknown_episodes_are_errors() {
    let (mut env, _) = Env::subscribed();
    assert!(matches!(
        env.podcasts.set_played(999, true),
        Err(Error::EpisodeNotFound(999))
    ));
    assert!(matches!(
        env.podcasts.set_position(999, 1),
        Err(Error::EpisodeNotFound(999))
    ));
}
