use std::sync::atomic::{AtomicBool, Ordering};

use unsub_podcasts::Error;

use crate::common::{Env, audio_bytes, fixture};

const BIG: usize = 2_000_000;

/// Runs a download that raises the cancel flag once `after` bytes have arrived.
fn cancelled_download(
    env: &mut Env,
    episode_id: i64,
    after: u64,
) -> Result<std::path::PathBuf, Error> {
    let cancel = AtomicBool::new(false);
    let mut progress = |done: u64, _: Option<u64>| {
        if done >= after {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    env.podcasts.download(episode_id, &mut progress, &cancel)
}

fn part_file(env: &Env, episode_id: i64) -> std::path::PathBuf {
    env.download_dir()
        .join("Test Cast")
        .join(format!("{episode_id}.part"))
}

/// Leaves a partial download behind as an interrupted run would, validator included.
fn plant_partial(env: &Env, episode_id: i64, bytes: &[u8], validator: Option<&str>) {
    let part = part_file(env, episode_id);
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    std::fs::write(&part, bytes).unwrap();
    if let Some(validator) = validator {
        std::fs::write(part.with_extension("validator"), validator).unwrap();
    }
}

#[test]
fn download_reports_progress_and_saves_the_file() {
    let (mut env, feed) = Env::subscribed();
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();
    assert_eq!(episode.title, "Episode 1");

    let mut calls = Vec::new();
    let path = env
        .podcasts
        .download(
            episode.id,
            &mut |done, total| calls.push((done, total)),
            &AtomicBool::new(false),
        )
        .unwrap();

    assert_eq!(
        path,
        env.download_dir().join("Test Cast").join("Episode 1.mp3")
    );
    assert_eq!(std::fs::read(&path).unwrap(), audio_bytes(200_000));
    assert!(!part_file(&env, episode.id).exists());

    assert_eq!(calls.first(), Some(&(0, Some(200_000))));
    assert_eq!(calls.last(), Some(&(200_000, Some(200_000))));
    assert!(
        calls.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "progress only moves forward"
    );
    assert!(
        calls.len() > 2,
        "large files report more than start and end"
    );

    let stored = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();
    assert_eq!(stored.file, Some(path));
}

#[test]
fn downloads_work_when_the_server_announces_no_length() {
    let (mut env, feed) = Env::subscribed();
    env.server.announce_length(false);
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();

    let mut calls = Vec::new();
    let path = env
        .podcasts
        .download(
            episode.id,
            &mut |done, total| calls.push((done, total)),
            &AtomicBool::new(false),
        )
        .unwrap();

    assert_eq!(std::fs::read(path).unwrap(), audio_bytes(200_000));
    assert!(calls.iter().all(|(_, total)| total.is_none()));
    assert_eq!(calls.last().map(|(done, _)| *done), Some(200_000));
}

#[test]
fn an_already_downloaded_episode_is_not_fetched_again() {
    let (mut env, feed) = Env::subscribed();
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();
    let first = env.download(episode.id);
    let second = env.download(episode.id);

    assert_eq!(first, second);
    assert_eq!(env.server.requests_for("/audio/ep1.mp3").len(), 1);
}

#[test]
fn a_file_deleted_behind_our_back_is_downloaded_again() {
    let (mut env, feed) = Env::subscribed();
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();
    let path = env.download(episode.id);
    std::fs::remove_file(&path).unwrap();

    assert_eq!(env.download(episode.id), path);
    assert!(path.exists());
    assert_eq!(env.server.requests_for("/audio/ep1.mp3").len(), 2);
}

#[test]
fn cancelling_keeps_the_partial_file_and_the_next_call_resumes_with_range() {
    let (mut env, feed) = Env::subscribed();
    env.server.serve_audio("/audio/ep1.mp3", BIG);
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();

    let result = cancelled_download(&mut env, episode.id, 100_000);
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    let partial = std::fs::metadata(part_file(&env, episode.id))
        .unwrap()
        .len();
    assert!(
        (100_000..BIG as u64).contains(&partial),
        "partial is {partial} bytes"
    );
    assert_eq!(
        env.podcasts.episodes(feed.id).unwrap().pop().unwrap().file,
        None
    );

    let mut first_call = None;
    let path = env
        .podcasts
        .download(
            episode.id,
            &mut |done, total| {
                first_call.get_or_insert((done, total));
            },
            &AtomicBool::new(false),
        )
        .unwrap();

    let requests = env.server.requests_for("/audio/ep1.mp3");
    assert_eq!(requests[0].range, None);
    assert_eq!(requests[1].range, Some(format!("bytes={partial}-")));
    assert_eq!(
        requests[1].if_range.as_deref(),
        Some("\"audio-2000000\""),
        "resume is conditional on the same file"
    );
    assert_eq!(first_call, Some((partial, Some(BIG as u64))));
    assert_eq!(std::fs::read(&path).unwrap(), audio_bytes(BIG));
    assert!(!part_file(&env, episode.id).exists());
}

#[test]
fn a_file_that_changed_on_the_server_is_restarted_instead_of_appended() {
    let (mut env, feed) = Env::subscribed();
    env.server.serve_audio("/audio/ep1.mp3", BIG);
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();
    assert!(matches!(
        cancelled_download(&mut env, episode.id, 100_000),
        Err(Error::Cancelled)
    ));
    let partial = std::fs::metadata(part_file(&env, episode.id))
        .unwrap()
        .len();

    // Same URL, new content: the old bytes must not end up in front of it.
    let replaced = BIG + 4_096;
    env.server.serve_audio("/audio/ep1.mp3", replaced);
    let mut first_call = None;
    let path = env
        .podcasts
        .download(
            episode.id,
            &mut |done, total| {
                first_call.get_or_insert((done, total));
            },
            &AtomicBool::new(false),
        )
        .unwrap();

    let retry = &env.server.requests_for("/audio/ep1.mp3")[1];
    assert_eq!(retry.range, Some(format!("bytes={partial}-")));
    assert_eq!(retry.if_range.as_deref(), Some("\"audio-2000000\""));
    assert_eq!(first_call, Some((0, Some(replaced as u64))));
    assert_eq!(std::fs::read(&path).unwrap(), audio_bytes(replaced));
}

#[test]
fn last_modified_guards_the_resume_when_there_is_no_etag() {
    let (mut env, feed) = Env::subscribed();
    let stamp = "Mon, 20 Jan 2025 09:00:00 GMT";
    env.server
        .serve_audio_with("/audio/ep1.mp3", BIG, None, Some(stamp));
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();
    assert!(matches!(
        cancelled_download(&mut env, episode.id, 100_000),
        Err(Error::Cancelled)
    ));

    let path = env.download(episode.id);
    let retry = &env.server.requests_for("/audio/ep1.mp3")[1];
    assert!(retry.range.is_some());
    assert_eq!(retry.if_range.as_deref(), Some(stamp));
    assert_eq!(std::fs::read(path).unwrap(), audio_bytes(BIG));
}

#[test]
fn a_server_that_offers_no_validator_is_never_resumed() {
    let (mut env, feed) = Env::subscribed();
    env.server
        .serve_audio_with("/audio/ep1.mp3", BIG, None, None);
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();
    assert!(matches!(
        cancelled_download(&mut env, episode.id, 100_000),
        Err(Error::Cancelled)
    ));

    let mut first_call = None;
    let path = env
        .podcasts
        .download(
            episode.id,
            &mut |done, total| {
                first_call.get_or_insert((done, total));
            },
            &AtomicBool::new(false),
        )
        .unwrap();

    let retry = &env.server.requests_for("/audio/ep1.mp3")[1];
    assert_eq!(
        (retry.range.as_deref(), retry.if_range.as_deref()),
        (None, None)
    );
    assert_eq!(first_call, Some((0, Some(BIG as u64))));
    assert_eq!(std::fs::read(path).unwrap(), audio_bytes(BIG));
}

#[test]
fn a_partial_file_with_no_recorded_validator_is_not_trusted() {
    let (mut env, feed) = Env::subscribed();
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();
    plant_partial(&env, episode.id, &[0xAA; 50_000], None);

    let path = env.download(episode.id);
    assert_eq!(env.server.requests_for("/audio/ep1.mp3")[0].range, None);
    assert_eq!(std::fs::read(path).unwrap(), audio_bytes(200_000));
}

#[test]
fn a_server_without_range_support_restarts_the_download() {
    let (mut env, feed) = Env::subscribed();
    env.server.serve_audio("/audio/ep1.mp3", BIG);
    env.server.support_ranges(false);
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();

    assert!(matches!(
        cancelled_download(&mut env, episode.id, 100_000),
        Err(Error::Cancelled)
    ));
    let partial = std::fs::metadata(part_file(&env, episode.id))
        .unwrap()
        .len();
    assert!(partial > 0);

    let mut first_call = None;
    let path = env
        .podcasts
        .download(
            episode.id,
            &mut |done, total| {
                first_call.get_or_insert((done, total));
            },
            &AtomicBool::new(false),
        )
        .unwrap();

    // The Range header was sent but ignored, so the stale bytes were thrown away.
    assert_eq!(
        env.server.requests_for("/audio/ep1.mp3")[1].range,
        Some(format!("bytes={partial}-"))
    );
    assert_eq!(first_call, Some((0, Some(BIG as u64))));
    assert_eq!(std::fs::read(&path).unwrap(), audio_bytes(BIG));
}

#[test]
fn a_complete_partial_file_is_finished_without_downloading_again() {
    let (mut env, feed) = Env::subscribed();
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();
    plant_partial(
        &env,
        episode.id,
        &audio_bytes(200_000),
        Some("\"audio-200000\""),
    );

    let path = env.download(episode.id);
    assert_eq!(std::fs::read(path).unwrap(), audio_bytes(200_000));
    assert_eq!(
        env.server.requests_for("/audio/ep1.mp3")[0]
            .range
            .as_deref(),
        Some("bytes=200000-")
    );
}

#[test]
fn a_stale_partial_file_longer_than_the_episode_is_discarded() {
    let (mut env, feed) = Env::subscribed();
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();
    plant_partial(
        &env,
        episode.id,
        &vec![0u8; 300_000],
        Some("\"audio-200000\""),
    );

    let path = env.download(episode.id);
    assert_eq!(std::fs::read(path).unwrap(), audio_bytes(200_000));
}

#[test]
fn cancelling_before_the_request_does_not_touch_the_network() {
    let (mut env, feed) = Env::subscribed();
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();

    let result = env
        .podcasts
        .download(episode.id, &mut |_, _| {}, &AtomicBool::new(true));
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(env.server.requests_for("/audio/ep1.mp3").is_empty());
}

#[test]
fn http_errors_are_reported_and_leave_no_file() {
    let (mut env, feed) = Env::subscribed();
    env.server.fail("/audio/ep1.mp3", 404);
    let episode = env.podcasts.episodes(feed.id).unwrap().pop().unwrap();

    let result = env
        .podcasts
        .download(episode.id, &mut |_, _| {}, &AtomicBool::new(false));
    assert!(
        matches!(result, Err(Error::Status { status: 404, .. })),
        "{result:?}"
    );
    assert_eq!(
        env.podcasts.episodes(feed.id).unwrap().pop().unwrap().file,
        None
    );
}

#[test]
fn unknown_episodes_are_an_error() {
    let mut env = Env::new();
    let result = env
        .podcasts
        .download(7, &mut |_, _| {}, &AtomicBool::new(false));
    assert!(matches!(result, Err(Error::EpisodeNotFound(7))));
}

#[test]
fn episodes_with_the_same_title_get_distinct_files() {
    let mut env = Env::new();
    let xml = fixture("feed.xml").replace("<title>Episode 2</title>", "<title>Episode 1</title>");
    env.server.serve_feed("/feed.xml", &xml);
    env.server.serve_audio("/audio/ep1.mp3", 20_000);
    env.server.serve_audio("/audio/ep2.mp3", 30_000);
    let feed = env
        .podcasts
        .subscribe(&env.server.url("/feed.xml"))
        .unwrap();

    let episodes = env.podcasts.episodes(feed.id).unwrap();
    let (ep2, ep1) = (&episodes[1], &episodes[2]);
    let first = env.download(ep1.id);
    let second = env.download(ep2.id);

    assert_ne!(first, second);
    assert_eq!(std::fs::read(first).unwrap(), audio_bytes(20_000));
    assert_eq!(std::fs::read(second).unwrap(), audio_bytes(30_000));
}

#[test]
fn awkward_titles_become_safe_file_names() {
    let mut env = Env::new();
    let xml = fixture("feed.xml")
        .replace(
            "<title>Episode 3</title>",
            "<title>Q&amp;A: Why? / \"Because\"</title>",
        )
        .replace("<title>Test Cast</title>", "<title>CON</title>");
    env.server.serve_feed("/feed.xml", &xml);
    env.server.serve_audio("/audio/ep3.mp3", 10_000);
    let feed = env
        .podcasts
        .subscribe(&env.server.url("/feed.xml"))
        .unwrap();

    let episode = env.podcasts.episodes(feed.id).unwrap().remove(0);
    let path = env.download(episode.id);
    assert_eq!(
        path,
        env.download_dir()
            .join("_CON")
            .join("Q&A_ Why_ _ _Because_.mp3")
    );
    assert!(path.exists());
}
