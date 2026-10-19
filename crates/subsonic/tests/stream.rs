//! `stream` and `download`: whole files, every Range form, readers, and failure modes.

mod common;

use std::thread;

use common::*;

const LEN: usize = SONG_FILE_LEN;

fn stream(h: &Harness, query: &str) -> Resp {
    h.get("stream", query)
}

fn ranged(h: &Harness, endpoint: &str, range: &str) -> Resp {
    h.get_with_header(&h.url(endpoint, "id=s-1"), "Range", range)
}

#[test]
fn whole_file_is_a_200_with_length_type_and_range_support_advertised() {
    let h = Harness::start();
    let reply = stream(&h, "id=s-1");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-length"), Some("10000"));
    assert_eq!(reply.header("content-type"), Some("audio/mpeg"));
    assert_eq!(reply.header("accept-ranges"), Some("bytes"));
    assert_eq!(reply.header("content-range"), None);
    assert_eq!(reply.body, file_bytes(0..LEN));
}

#[test]
fn explicit_range_is_a_206_with_content_range() {
    let h = Harness::start();
    let reply = ranged(&h, "stream", "bytes=100-199");
    assert_eq!(reply.status, 206);
    assert_eq!(reply.header("content-range"), Some("bytes 100-199/10000"));
    assert_eq!(reply.header("content-length"), Some("100"));
    assert_eq!(reply.header("accept-ranges"), Some("bytes"));
    assert_eq!(reply.header("content-type"), Some("audio/mpeg"));
    assert_eq!(reply.body, file_bytes(100..200));
}

#[test]
fn open_ended_range_runs_to_the_end() {
    let h = Harness::start();
    let reply = ranged(&h, "stream", "bytes=9990-");
    assert_eq!(reply.status, 206);
    assert_eq!(reply.header("content-range"), Some("bytes 9990-9999/10000"));
    assert_eq!(reply.body, file_bytes(9990..LEN));
}

#[test]
fn zero_to_open_end_is_a_206_covering_everything() {
    let h = Harness::start();
    let reply = ranged(&h, "stream", "bytes=0-");
    assert_eq!(reply.status, 206);
    assert_eq!(reply.header("content-range"), Some("bytes 0-9999/10000"));
    assert_eq!(reply.body, file_bytes(0..LEN));
}

#[test]
fn suffix_range_is_the_last_n_bytes() {
    let h = Harness::start();
    let reply = ranged(&h, "stream", "bytes=-500");
    assert_eq!(reply.status, 206);
    assert_eq!(reply.header("content-range"), Some("bytes 9500-9999/10000"));
    assert_eq!(reply.body, file_bytes(9500..LEN));

    let reply = ranged(&h, "stream", "bytes=-99999");
    assert_eq!((reply.status, reply.body.len()), (206, LEN));
}

#[test]
fn end_past_the_file_is_clamped() {
    let h = Harness::start();
    let reply = ranged(&h, "stream", "bytes=9000-999999");
    assert_eq!(reply.status, 206);
    assert_eq!(reply.header("content-range"), Some("bytes 9000-9999/10000"));
    assert_eq!(reply.body, file_bytes(9000..LEN));
}

#[test]
fn single_byte_ranges_at_both_ends() {
    let h = Harness::start();
    let first = ranged(&h, "stream", "bytes=0-0");
    assert_eq!((first.status, first.body), (206, vec![file_byte(0)]));
    let last = ranged(&h, "stream", "bytes=9999-9999");
    assert_eq!((last.status, last.body), (206, vec![file_byte(9999)]));
}

#[test]
fn unsatisfiable_ranges_are_416_with_the_length() {
    let h = Harness::start();
    for range in [
        "bytes=10000-",
        "bytes=10000-10010",
        "bytes=20000-30000",
        "bytes=-0",
    ] {
        let reply = ranged(&h, "stream", range);
        assert_eq!(reply.status, 416, "{range}");
        assert_eq!(
            reply.header("content-range"),
            Some("bytes */10000"),
            "{range}"
        );
        assert!(reply.body.is_empty(), "{range}");
    }
}

#[test]
fn ranges_the_server_cannot_honour_fall_back_to_the_whole_file() {
    let h = Harness::start();
    for range in [
        "bytes=500-100",
        "bytes=abc-def",
        "items=0-5",
        "bytes=0-5,10-15",
        "bytes=",
        "garbage",
    ] {
        let reply = ranged(&h, "stream", range);
        assert_eq!(reply.status, 200, "{range}");
        assert_eq!(reply.body, file_bytes(0..LEN), "{range}");
    }
}

#[test]
fn consecutive_ranges_reassemble_the_file() {
    let h = Harness::start();
    let mut assembled = Vec::new();
    for start in (0..LEN).step_by(3333) {
        let end = (start + 3332).min(LEN - 1);
        let reply = ranged(&h, "stream", &format!("bytes={start}-{end}"));
        assert_eq!(reply.status, 206);
        assembled.extend(reply.body);
    }
    assert_eq!(assembled, file_bytes(0..LEN));
}

#[test]
fn empty_file_streams_as_empty_and_rejects_any_range() {
    let h = Harness::start();
    let reply = stream(&h, "id=s-4");
    assert_eq!((reply.status, reply.body.len()), (200, 0));
    assert_eq!(reply.header("content-length"), Some("0"));
    let reply = h.get_with_header(&h.url("stream", "id=s-4"), "Range", "bytes=0-");
    assert_eq!(reply.status, 416);
    assert_eq!(reply.header("content-range"), Some("bytes */0"));
}

#[test]
fn download_serves_the_original_file_with_ranges_and_a_filename() {
    let h = Harness::start();
    let reply = h.get("download", "id=s-1&maxBitRate=64&format=mp3");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, file_bytes(0..LEN));
    assert_eq!(
        reply.header("content-disposition"),
        Some("attachment; filename=\"song1.mp3\"; filename*=UTF-8''song1%2Emp3")
    );
    // The source is asked for the raw file whatever transcoding the client requested.
    assert_eq!(
        h.source.calls.lock().unwrap().open_stream,
        [("s-1".to_owned(), None, None)]
    );

    let reply = ranged(&h, "download", "bytes=10-19");
    assert_eq!((reply.status, reply.body), (206, file_bytes(10..20)));
    assert_eq!(ranged(&h, "download", "bytes=99999-").status, 416);
}

#[test]
fn download_encodes_non_ascii_file_names() {
    let h = Harness::start();
    let reply = h.get("download", "id=s-7");
    assert_eq!(
        reply.header("content-disposition"),
        Some(
            "attachment; filename=\"_n_code_ name.mp3\"; filename*=UTF-8''%C3%9Cn%C3%AFcode%2C%20name%2Emp3"
        )
    );
}

#[test]
fn stream_has_no_content_disposition() {
    let h = Harness::start();
    assert_eq!(stream(&h, "id=s-1").header("content-disposition"), None);
}

#[test]
fn bitrate_and_format_reach_the_source_and_raw_means_untouched() {
    let h = Harness::start();
    stream(&h, "id=s-1&maxBitRate=128&format=mp3");
    stream(&h, "id=s-1&maxBitRate=0&format=raw");
    stream(&h, "id=s-1&format=RAW");
    stream(&h, "id=s-1");
    let calls = &h.source.calls.lock().unwrap().open_stream;
    let id = |bitrate, format: Option<&str>| ("s-1".to_owned(), bitrate, format.map(str::to_owned));
    assert_eq!(
        *calls,
        [
            id(Some(128), Some("mp3")),
            id(None, None),
            id(None, None),
            id(None, None)
        ]
    );
}

#[test]
fn reader_streams_are_sent_whole_with_their_length_and_ignore_ranges() {
    let h = Harness::start();
    let reply = stream(&h, "id=s-3&maxBitRate=96&format=ogg");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-type"), Some("audio/ogg"));
    assert_eq!(reply.header("content-length"), Some("100000"));
    assert_eq!(reply.header("accept-ranges"), None);
    assert_eq!(reply.body, file_bytes(0..TRANSCODE_LEN));

    let reply = h.get_with_header(&h.url("stream", "id=s-3"), "Range", "bytes=10-19");
    assert_eq!((reply.status, reply.body.len()), (200, TRANSCODE_LEN));
}

#[test]
fn unknown_song_is_404_with_error_70() {
    let h = Harness::start();
    for endpoint in ["stream", "download"] {
        for format in ["", "&f=json"] {
            let reply = h.get(endpoint, &format!("id=nope{format}"));
            assert_eq!(reply.status, 404, "{endpoint}");
            assert_eq!(error_of(&reply).0, 70, "{endpoint}");
        }
    }
}

#[test]
fn file_missing_from_disk_is_404_with_error_70() {
    let h = Harness::start();
    let reply = stream(&h, "id=s-2");
    assert_eq!(reply.status, 404);
    assert_eq!(error_of(&reply).0, 70);
}

#[test]
fn source_failure_while_opening_is_a_generic_error() {
    let h = Harness::start();
    let reply = stream(&h, "id=broken");
    assert_eq!(reply.status, 200);
    assert_eq!(error_of(&reply), (0, "disk on fire".to_owned()));
}

#[test]
fn view_suffix_and_post_work_for_stream() {
    let h = Harness::start();
    let reply = h.get("stream.view", "id=s-1");
    assert_eq!(reply.body, file_bytes(0..LEN));
    let reply = h.post(
        "stream",
        &[
            ("u", USER),
            ("p", PASSWORD),
            ("v", "1.16.1"),
            ("c", "t"),
            ("id", "s-1"),
        ],
    );
    assert_eq!(reply.body, file_bytes(0..LEN));
}

#[test]
fn concurrent_clients_each_get_their_own_correct_bytes() {
    let h = Harness::start();
    let workers: Vec<_> = (0..12)
        .map(|i| {
            let (url, agent) = (h.url("stream", "id=s-1"), agent());
            thread::spawn(move || {
                let start = i * 700;
                let mut response = agent
                    .get(&url)
                    .header("Range", format!("bytes={start}-{}", start + 999))
                    .call()
                    .unwrap();
                assert_eq!(response.status().as_u16(), 206);
                (start, response.body_mut().read_to_vec().unwrap())
            })
        })
        .collect();
    for worker in workers {
        let (start, body) = worker.join().unwrap();
        assert_eq!(body, file_bytes(start..start + 1000));
    }
}
