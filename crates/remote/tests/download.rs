mod common;

use std::fs;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use common::{FakeServer, PASSWORD, USER, pattern};
use filetime::{FileTime, set_file_mtime};
use unsub_remote::{Client, Error};

fn connect(server: &FakeServer) -> Client {
    Client::new(&server.url, USER, PASSWORD)
}

/// A server with song `s-1` of 600,000 bytes that supports resuming.
fn server_with_song() -> (FakeServer, Vec<u8>) {
    let server = FakeServer::start();
    let data = pattern(600_000, 1);
    server.song("s-1", data.clone(), "audio/flac");
    server.mode(|mode| mode.honor_range = true);
    (server, data)
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn file_ending(dir: &Path, suffix: &str) -> PathBuf {
    let found: Vec<_> = names(dir)
        .into_iter()
        .filter(|name| name.ends_with(suffix))
        .collect();
    assert_eq!(found.len(), 1, "{suffix}: {:?}", names(dir));
    dir.join(&found[0])
}

/// Leaves a `.part` file behind by letting a download break part of the way through.
fn interrupt(server: &FakeServer, client: &Client, dir: &Path, after: usize) -> PathBuf {
    server.mode(|mode| mode.cut_after = Some(after));
    assert!(client.download_to_cache("s-1", dir).is_err());
    server.mode(|mode| mode.cut_after = None);
    // The song's type is in the name from the start, so a crash does not lose it.
    file_ending(dir, ".flac.part")
}

fn hours_ago(path: &Path, hours: u64) {
    let when = SystemTime::now() - Duration::from_secs(hours * 3600);
    set_file_mtime(path, FileTime::from_system_time(when)).unwrap();
}

#[test]
fn downloads_the_original_file_and_names_it_by_its_type() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();

    let path = connect(&server)
        .download_to_cache("s-1", dir.path())
        .unwrap();

    assert_eq!(fs::read(&path).unwrap(), data);
    let name = path.file_name().unwrap().to_str().unwrap();
    let (stem, extension) = name.split_once('.').unwrap();
    assert_eq!(extension, "flac");
    assert!(
        stem.len() == 32 && stem.bytes().all(|b| b.is_ascii_hexdigit()),
        "{name}"
    );
    assert_eq!(names(dir.path()), [name], "no partial file is left behind");

    let requests = server.requests_to("stream");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].params["id"], "s-1");
    assert_eq!(
        requests[0].params["format"], "raw",
        "the bytes must be stable to resume"
    );
    assert!(!requests[0].params.contains_key("maxBitRate"));
    assert_eq!(requests[0].range, None);
}

#[test]
fn a_cached_song_is_returned_without_asking_the_server() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server);
    let first = client.download_to_cache("s-1", dir.path()).unwrap();

    let download = client.start_download("s-1", dir.path()).unwrap();

    assert_eq!(download.path(), first);
    let progress = download.progress();
    assert!(progress.finished);
    assert_eq!(progress.total, Some(data.len() as u64));
    let mut played = Vec::new();
    download.reader().unwrap().read_to_end(&mut played).unwrap();
    assert_eq!(played, data);
    assert_eq!(server.requests_to("stream").len(), 1);
}

#[test]
fn songs_from_different_servers_do_not_share_a_cache_entry() {
    let (one, first) = server_with_song();
    let (two, _) = server_with_song();
    two.song("s-1", pattern(1000, 9), "audio/mpeg");
    let dir = tempfile::tempdir().unwrap();

    let a = connect(&one).download_to_cache("s-1", dir.path()).unwrap();
    let b = connect(&two).download_to_cache("s-1", dir.path()).unwrap();

    assert_ne!(a, b);
    assert_eq!(fs::read(a).unwrap(), first);
    assert_eq!(fs::read(b).unwrap(), pattern(1000, 9));
}

#[test]
fn resumes_an_interrupted_download_where_it_stopped() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server);
    let part = interrupt(&server, &client, dir.path(), 250_000);
    let have = fs::metadata(&part).unwrap().len();
    assert!(have > 0 && have < data.len() as u64, "{have}");
    assert_eq!(
        fs::read(&part).unwrap(),
        data[..have as usize],
        "what arrived is intact"
    );

    let path = client.download_to_cache("s-1", dir.path()).unwrap();

    assert_eq!(fs::read(&path).unwrap(), data);
    assert!(!part.exists());
    let requests = server.requests_to("stream");
    assert_eq!(
        requests.last().unwrap().range,
        Some(format!("bytes={have}-"))
    );
}

#[test]
fn a_server_that_ignores_ranges_gets_a_fresh_start() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server);
    let part = interrupt(&server, &client, dir.path(), 250_000);
    fs::write(&part, vec![0xAA; 1000]).unwrap();
    server.mode(|mode| mode.honor_range = false);

    let path = client.download_to_cache("s-1", dir.path()).unwrap();

    assert_eq!(
        fs::read(path).unwrap(),
        data,
        "the old bytes were thrown away"
    );
    assert_eq!(
        server.requests_to("stream").last().unwrap().range,
        Some("bytes=1000-".to_owned())
    );
}

#[test]
fn a_partial_file_that_already_holds_the_whole_song_is_finished_at_once() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server);
    let part = interrupt(&server, &client, dir.path(), 250_000);
    fs::write(&part, &data).unwrap();

    let path = client.download_to_cache("s-1", dir.path()).unwrap();

    assert_eq!(fs::read(&path).unwrap(), data);
    assert_eq!(
        path.extension().unwrap(),
        "flac",
        "the type survives the interruption"
    );
    assert!(!part.exists());
    assert_eq!(names(dir.path()).len(), 1);
}

#[test]
fn a_partial_file_longer_than_the_song_is_replaced() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server);
    let part = interrupt(&server, &client, dir.path(), 250_000);
    fs::write(&part, pattern(data.len() * 3, 5)).unwrap();

    let path = client.download_to_cache("s-1", dir.path()).unwrap();

    assert_eq!(fs::read(path).unwrap(), data);
    let requests = server.requests_to("stream");
    let ranges: Vec<_> = requests
        .iter()
        .rev()
        .take(2)
        .map(|request| request.range.clone())
        .collect();
    assert_eq!(ranges, [None, Some(format!("bytes={}-", data.len() * 3))]);
}

#[test]
fn a_song_bigger_than_the_whole_cache_is_refused() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server).with_cache_limit(100_000);

    let result = client.download_to_cache("s-1", dir.path());

    match result {
        Err(Error::TooLargeForCache { size, limit }) => {
            assert_eq!((size, limit), (data.len() as u64, 100_000));
        }
        other => panic!("{other:?}"),
    }
    assert!(names(dir.path()).is_empty());
}

#[test]
fn a_song_of_unknown_length_cannot_overfill_the_cache() {
    let (server, _) = server_with_song();
    server.mode(|mode| mode.chunked = true);
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server).with_cache_limit(50_000);

    let result = client.download_to_cache("s-1", dir.path());

    assert!(
        matches!(result, Err(Error::TooLargeForCache { limit: 50_000, .. })),
        "{result:?}"
    );
    assert!(
        names(dir.path()).is_empty(),
        "the oversized partial file is deleted"
    );
}

#[test]
fn the_least_recently_used_songs_are_deleted_to_stay_under_the_limit() {
    let server = FakeServer::start();
    for (id, seed) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)] {
        server.song(id, pattern(100_000, seed), "audio/mpeg");
    }
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server).with_cache_limit(250_000);
    let a = client.download_to_cache("a", dir.path()).unwrap();
    let b = client.download_to_cache("b", dir.path()).unwrap();
    hours_ago(&a, 2);
    hours_ago(&b, 1);

    let c = client.download_to_cache("c", dir.path()).unwrap();

    assert!(!a.exists(), "the oldest went");
    assert!(b.exists() && c.exists());

    // Playing b again makes c the one nobody has wanted lately.
    hours_ago(&c, 1);
    client.download_to_cache("b", dir.path()).unwrap();
    let d = client.download_to_cache("d", dir.path()).unwrap();

    assert!(!c.exists());
    assert!(b.exists() && d.exists());
    assert_eq!(
        server.requests_to("stream").len(),
        4,
        "the repeat of b came from the cache"
    );
}

#[test]
fn problems_are_reported_before_any_file_is_made() {
    let (server, _) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server);

    assert!(matches!(
        client.start_download("nope", dir.path()),
        Err(Error::NotFound)
    ));
    server.mode(|mode| mode.missing_as_404 = true);
    assert!(matches!(
        client.start_download("nope", dir.path()),
        Err(Error::NotFound)
    ));
    let stranger = Client::new(&server.url, USER, "wrong");
    assert!(matches!(
        stranger.start_download("s-1", dir.path()),
        Err(Error::WrongCredentials)
    ));

    assert!(names(dir.path()).is_empty());
}

#[test]
fn playback_can_read_while_the_download_is_still_arriving() {
    let server = FakeServer::start();
    let data = pattern(400_000, 7);
    server.song("s-1", data.clone(), "audio/mpeg");
    let (permits, gate) = channel();
    server.mode(|mode| mode.gate = Some(gate));
    let dir = tempfile::tempdir().unwrap();

    let download = connect(&server).start_download("s-1", dir.path()).unwrap();
    let progress = download.progress();
    assert_eq!(
        (progress.downloaded, progress.total, progress.finished),
        (0, Some(400_000), false)
    );
    assert_eq!(download.path().extension().unwrap(), "part");

    // The first 100 kB arrive. A little less is asked for because the server may hold back
    // the last few kilobytes in a buffer.
    permits.send(100_000).unwrap();
    download.wait_for(64_000).unwrap();
    assert!(!download.progress().finished);
    let mut reader = download.reader().unwrap();
    let mut head = vec![0; 64_000];
    reader.read_exact(&mut head).unwrap();
    assert_eq!(head, data[..64_000]);

    // Reading on has to wait for the rest.
    let rest = thread::spawn(move || {
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).map(|_| rest)
    });
    thread::sleep(Duration::from_millis(300));
    assert!(
        !rest.is_finished(),
        "the reader must wait, not report the end of the file"
    );
    permits.send(300_000).unwrap();
    let rest = rest.join().unwrap().unwrap();

    assert_eq!([head, rest].concat(), data);
    let path = download.wait().unwrap();
    assert_eq!(path.extension().unwrap(), "mp3");
    assert_eq!(fs::read(path).unwrap(), data);
}

#[test]
fn a_reader_can_seek_and_knows_the_length_from_the_start() {
    let server = FakeServer::start();
    let data = pattern(300_000, 3);
    server.song("s-1", data.clone(), "audio/mpeg");
    let (permits, gate) = channel();
    server.mode(|mode| mode.gate = Some(gate));
    let dir = tempfile::tempdir().unwrap();
    let download = connect(&server).start_download("s-1", dir.path()).unwrap();
    let mut reader = download.reader().unwrap();

    assert_eq!(reader.len(), Some(300_000));
    assert_eq!(reader.seek(SeekFrom::End(-10)).unwrap(), 299_990);
    assert_eq!(reader.seek(SeekFrom::Current(-5)).unwrap(), 299_985);
    assert_eq!(reader.seek(SeekFrom::Start(299_990)).unwrap(), 299_990);
    let error = reader.seek(SeekFrom::Current(-300_000)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidInput);

    // Reading the tail waits until the whole song is here.
    permits.send(300_000).unwrap();
    let mut tail = [0; 10];
    reader.read_exact(&mut tail).unwrap();
    assert_eq!(tail, data[299_990..]);
    reader.seek(SeekFrom::Start(100)).unwrap();
    reader.read_exact(&mut tail).unwrap();
    assert_eq!(tail, data[100..110]);
    let mut end = Vec::new();
    reader.seek(SeekFrom::End(0)).unwrap();
    assert_eq!(
        reader.read_to_end(&mut end).unwrap(),
        0,
        "the end is the end once it is done"
    );
}

#[test]
fn a_song_of_unknown_length_cannot_be_sought_from_the_end_until_it_is_done() {
    let server = FakeServer::start();
    let data = pattern(100_000, 3);
    server.song("s-1", data.clone(), "audio/mpeg");
    let (permits, gate) = channel();
    server.mode(|mode| {
        mode.gate = Some(gate);
        mode.chunked = true;
    });
    let dir = tempfile::tempdir().unwrap();
    let download = connect(&server).start_download("s-1", dir.path()).unwrap();
    let mut reader = download.reader().unwrap();

    assert_eq!(reader.len(), None);
    assert_eq!(
        reader.seek(SeekFrom::End(0)).unwrap_err().kind(),
        ErrorKind::Unsupported
    );

    permits.send(100_000).unwrap();
    download.wait().unwrap();
    assert_eq!(reader.len(), Some(100_000));
    assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), 100_000);
}

#[test]
fn a_failed_download_fails_its_readers_and_waiters_but_leaves_a_resumable_file() {
    let (server, data) = server_with_song();
    let (permits, gate) = channel();
    server.mode(|mode| mode.gate = Some(gate));
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server);
    let download = client.start_download("s-1", dir.path()).unwrap();
    let mut reader = download.reader().unwrap();

    permits.send(100_000).unwrap();
    download.wait_for(64_000).unwrap();
    drop(permits); // the connection drops

    let mut everything = Vec::new();
    let error = reader.read_to_end(&mut everything).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Other, "{error}");
    assert!(matches!(
        download.wait_for(500_000),
        Err(Error::DownloadFailed(_))
    ));
    assert!(download.wait().is_err());

    let part = file_ending(dir.path(), ".part");
    assert!(fs::metadata(&part).unwrap().len() >= 64_000);
    server.mode(|mode| mode.honor_range = true);
    assert_eq!(
        fs::read(client.download_to_cache("s-1", dir.path()).unwrap()).unwrap(),
        data
    );
}

#[test]
fn cancelling_stops_the_download_and_keeps_what_arrived() {
    let (server, data) = server_with_song();
    let (permits, gate) = channel();
    server.mode(|mode| mode.gate = Some(gate));
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server);
    let download = client.start_download("s-1", dir.path()).unwrap();

    permits.send(100_000).unwrap();
    download.wait_for(64_000).unwrap();
    download.cancel();
    permits.send(50_000).unwrap(); // lets the thread come round and notice
    let error = download.wait().unwrap_err();

    assert!(error.to_string().contains("cancelled"), "{error}");
    let part = file_ending(dir.path(), ".part");
    let have = fs::metadata(&part).unwrap().len();
    assert!(have >= 64_000 && have < data.len() as u64);

    drop(permits);
    let path = client.download_to_cache("s-1", dir.path()).unwrap();
    assert_eq!(fs::read(path).unwrap(), data);
    assert_eq!(
        server.requests_to("stream").last().unwrap().range,
        Some(format!("bytes={have}-"))
    );
}

#[test]
fn progress_serializes_for_the_ui() {
    let (server, _) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let download = connect(&server).start_download("s-1", dir.path()).unwrap();
    download.wait_for(600_000).unwrap();

    let json = serde_json::to_value(download.progress()).unwrap();

    assert_eq!(json["downloaded"], 600_000);
    assert_eq!(json["total"], 600_000);
    assert!(json.get("finished").is_some());
}

/// Waits until a file stops growing, which is when the download thread behind it has stopped.
fn settled_size(path: &Path) -> u64 {
    let mut last = u64::MAX;
    for _ in 0..100 {
        let size = fs::metadata(path).map_or(0, |metadata| metadata.len());
        if size == last {
            return size;
        }
        last = size;
        thread::sleep(Duration::from_millis(100));
    }
    panic!("{} never stopped growing", path.display());
}

fn gated_download(
    server: &FakeServer,
    dir: &Path,
) -> (unsub_remote::Download, std::sync::mpsc::Sender<usize>) {
    let (permits, gate) = channel();
    server.mode(|mode| mode.gate = Some(gate));
    let download = connect(server).start_download("s-1", dir).unwrap();
    permits.send(100_000).unwrap();
    download.wait_for(64_000).unwrap();
    (download, permits)
}

#[test]
fn dropping_a_download_cancels_it_and_keeps_what_arrived() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let (download, permits) = gated_download(&server, dir.path());
    let part = download.path();

    drop(download);
    // Plenty more is on offer, but nobody is taking it.
    permits.send(600_000).unwrap();

    let kept = settled_size(&part);
    assert!(kept >= 64_000 && kept < data.len() as u64, "{kept}");
    assert_eq!(names(dir.path()).len(), 1, "no finished file appeared");
    drop(permits);
    let path = connect(&server)
        .download_to_cache("s-1", dir.path())
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), data);
    assert_eq!(
        server.requests_to("stream").last().unwrap().range,
        Some(format!("bytes={kept}-"))
    );
}

#[test]
fn a_detached_download_finishes_with_nobody_waiting() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let (download, permits) = gated_download(&server, dir.path());

    download.detach();
    permits.send(600_000).unwrap();

    let started = Instant::now();
    let finished = loop {
        let done: Vec<_> = names(dir.path())
            .into_iter()
            .filter(|name| name.ends_with(".flac"))
            .collect();
        if let Some(name) = done.first() {
            break dir.path().join(name);
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it never finished"
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(fs::read(finished).unwrap(), data);
}

#[test]
fn a_download_lives_as_long_as_a_reader_taken_from_it() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let (download, permits) = gated_download(&server, dir.path());
    let mut reader = download.reader().unwrap();

    drop(download); // playback goes on
    permits.send(600_000).unwrap();
    let mut played = Vec::new();
    reader.read_to_end(&mut played).unwrap();

    assert_eq!(played, data);
    drop(reader);
    assert!(
        names(dir.path())[0].ends_with(".flac"),
        "{:?}",
        names(dir.path())
    );
}

#[test]
fn dropping_the_last_reader_cancels_a_dropped_download() {
    let (server, data) = server_with_song();
    let dir = tempfile::tempdir().unwrap();
    let (download, permits) = gated_download(&server, dir.path());
    let part = download.path();
    let reader = download.reader().unwrap();
    drop(download);

    drop(reader);
    permits.send(600_000).unwrap();

    let kept = settled_size(&part);
    assert!(kept < data.len() as u64, "{kept}");
    assert_eq!(names(dir.path()).len(), 1);
    assert!(names(dir.path())[0].ends_with(".flac.part"));
}

#[test]
fn a_server_that_goes_quiet_mid_song_stalls_the_download_and_it_can_resume() {
    let (server, data) = server_with_song();
    server.mode(|mode| mode.stall_after = Some(200_000));
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server).with_stall_timeout(Duration::from_millis(400));

    let started = Instant::now();
    let result = client.download_to_cache("s-1", dir.path());

    assert!(matches!(result, Err(Error::Stalled)), "{result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it must not wait for the server"
    );
    let part = file_ending(dir.path(), ".flac.part");
    let have = fs::metadata(&part).unwrap().len();
    assert!(have > 0 && have < data.len() as u64, "{have}");

    server.mode(|mode| mode.stall_after = None);
    let path = client.download_to_cache("s-1", dir.path()).unwrap();
    assert_eq!(fs::read(path).unwrap(), data);
    assert_eq!(
        server.requests_to("stream").last().unwrap().range,
        Some(format!("bytes={have}-"))
    );
}

#[test]
fn a_server_that_never_answers_stalls_the_request() {
    let (server, _) = server_with_song();
    server.mode(|mode| mode.silent = true);
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&server).with_stall_timeout(Duration::from_millis(300));

    let started = Instant::now();
    let result = client.start_download("s-1", dir.path());

    assert!(matches!(result, Err(Error::Stalled)), "{:?}", result.err());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(names(dir.path()).is_empty());
}

#[test]
fn a_slow_but_steady_download_is_never_cut_off() {
    let server = FakeServer::start();
    let data = pattern(300_000, 2);
    server.song("s-1", data.clone(), "audio/mpeg");
    let (permits, gate) = channel();
    server.mode(|mode| mode.gate = Some(gate));
    let dir = tempfile::tempdir().unwrap();
    let stall = Duration::from_millis(400);
    let client = connect(&server).with_stall_timeout(stall);

    // Six pieces, each arriving well inside the stall timeout, together taking far longer than it.
    let feeder = thread::spawn(move || {
        for _ in 0..6 {
            permits.send(50_000).unwrap();
            thread::sleep(Duration::from_millis(150));
        }
    });
    let started = Instant::now();
    let path = client.download_to_cache("s-1", dir.path()).unwrap();
    feeder.join().unwrap();

    assert!(
        started.elapsed() > stall,
        "the whole transfer outlasted the stall timeout"
    );
    assert_eq!(fs::read(path).unwrap(), data);
}
