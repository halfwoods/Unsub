use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use antistreaming_podcasts::Error;

use crate::common::{Env, audio_bytes};

const BIG: usize = 2_000_000;

fn part_len(env: &Env, episode_id: i64) -> u64 {
    let part = env
        .download_dir()
        .join("Test Cast")
        .join(format!("{episode_id}.part"));
    std::fs::metadata(part).map_or(0, |metadata| metadata.len())
}

#[test]
fn a_server_that_goes_quiet_mid_body_stalls_the_download_and_the_next_call_resumes() {
    let (mut env, feed) = Env::subscribed();
    env.podcasts.set_stall_timeout(Duration::from_millis(200));
    env.server
        .serve_stalling_audio("/audio/ep1.mp3", BIG, 300_000);
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();

    let started = Instant::now();
    let result = env
        .podcasts
        .download(episode.id, &mut |_, _| {}, &AtomicBool::new(false));
    assert!(matches!(result, Err(Error::Stalled)), "{result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "gave up after {:?}",
        started.elapsed()
    );

    let partial = part_len(&env, episode.id);
    assert!(
        (100_000..BIG as u64).contains(&partial),
        "partial is {partial} bytes"
    );
    assert_eq!(
        env.podcasts.episodes(feed.id).unwrap().pop().unwrap().file,
        None
    );

    // The server recovers; the same file resumes where it stopped.
    env.server.serve_audio("/audio/ep1.mp3", BIG);
    let path = env.download(episode.id);
    let retry = &env.server.requests_for("/audio/ep1.mp3")[1];
    assert_eq!(retry.range, Some(format!("bytes={partial}-")));
    assert_eq!(std::fs::read(path).unwrap(), audio_bytes(BIG));
}

#[test]
fn a_server_that_never_starts_sending_stalls_too() {
    let (mut env, feed) = Env::subscribed();
    env.podcasts.set_stall_timeout(Duration::from_millis(200));
    env.server.serve_stalling_audio("/audio/ep1.mp3", BIG, 0);
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();

    let started = Instant::now();
    let result = env
        .podcasts
        .download(episode.id, &mut |_, _| {}, &AtomicBool::new(false));
    assert!(matches!(result, Err(Error::Stalled)), "{result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "gave up after {:?}",
        started.elapsed()
    );
}

#[test]
fn total_duration_is_not_limited_while_data_keeps_arriving() {
    let (mut env, feed) = Env::subscribed();
    env.podcasts.set_stall_timeout(Duration::from_millis(400));
    // The server writes in 8 KiB steps: thirteen pauses of 100 ms make about 1.3 s in all,
    // more than three times the stall timeout.
    env.server
        .serve_trickling_audio("/audio/ep1.mp3", 100_000, 8_192, Duration::from_millis(100));
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();

    let started = Instant::now();
    let path = env.download(episode.id);
    assert!(
        started.elapsed() > Duration::from_millis(800),
        "finished in {:?}",
        started.elapsed()
    );
    assert_eq!(std::fs::read(path).unwrap(), audio_bytes(100_000));
}

#[test]
fn feed_requests_also_give_up_on_a_silent_server() {
    let (mut env, feed) = Env::subscribed();
    env.podcasts.set_stall_timeout(Duration::from_millis(200));
    env.server.serve_stalling_audio("/silent.xml", BIG, 0);

    let started = Instant::now();
    assert!(
        env.podcasts
            .subscribe(&env.server.url("/silent.xml"))
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        env.podcasts.feeds().unwrap().len(),
        1,
        "the earlier subscription is untouched ({})",
        feed.title
    );
}
