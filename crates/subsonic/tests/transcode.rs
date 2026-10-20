//! Streaming converted audio: which requests are converted, how the answer is sent, and what happens
//! when the source cannot.

mod common;

use std::{
    io::{self, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use antistreaming_subsonic::StreamFormat;
use common::*;

/// What the server asked the source to convert, in order.
fn asked(h: &Harness) -> Vec<(String, StreamFormat, Option<u32>)> {
    h.source.calls.lock().unwrap().transcoded.clone()
}

fn converting(default_max: Option<u32>) -> Harness {
    Harness::start_converting(default_max, Transcode::Canned)
}

/// The song as stored: s-1, s-9 and s-10 all serve the same test file.
fn assert_original(reply: &Resp) {
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-type"), Some("audio/mpeg"));
    assert_eq!(reply.header("accept-ranges"), Some("bytes"));
    assert_eq!(reply.body, file_bytes(0..SONG_FILE_LEN));
}

fn assert_converted(reply: &Resp, format: StreamFormat, max_kbps: Option<u32>) {
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-type"), Some(canned_mime(format)));
    assert_eq!(
        reply.body,
        canned(format, max_kbps),
        "{format:?} at {max_kbps:?}"
    );
}

#[test]
fn with_no_format_or_bitrate_the_original_is_streamed_with_ranges() {
    let h = converting(None);
    assert_original(&h.get("stream", "id=s-1"));
    let ranged = h.get_with_header(&h.url("stream", "id=s-1"), "Range", "bytes=10-19");
    assert_eq!((ranged.status, ranged.body), (206, file_bytes(10..20)));
    assert!(asked(&h).is_empty());
}

#[test]
fn raw_streams_the_original_whatever_else_is_set() {
    let h = converting(Some(64));
    for query in [
        "id=s-9&format=raw",
        "id=s-9&format=RAW&maxBitRate=32",
        "id=s-9&format=raw&maxBitRate=0",
    ] {
        assert_original(&h.get("stream", query));
    }
    assert!(asked(&h).is_empty());
}

#[test]
fn each_format_is_converted_and_sent_chunked_without_length_or_ranges() {
    let h = converting(None);
    for (param, format, song) in [
        ("mp3", StreamFormat::Mp3, "s-9"),
        ("aac", StreamFormat::Aac, "s-9"),
        ("opus", StreamFormat::Opus, "s-9"),
        ("flac", StreamFormat::Flac, "s-1"),
        ("m4a", StreamFormat::Aac, "s-9"),
        ("ogg", StreamFormat::Opus, "s-9"),
        ("MP3", StreamFormat::Mp3, "s-9"),
    ] {
        let reply = h.get("stream", &format!("id={song}&format={param}"));
        assert_converted(&reply, format, None);
        assert_eq!(reply.header("content-length"), None, "{param}");
        assert_eq!(
            reply.header("transfer-encoding"),
            Some("chunked"),
            "{param}"
        );
        assert_eq!(reply.header("accept-ranges"), None, "{param}");
        assert_eq!(reply.header("content-disposition"), None, "{param}");
    }
    let formats: Vec<_> = asked(&h).into_iter().map(|(_, format, _)| format).collect();
    assert_eq!(
        formats,
        [
            StreamFormat::Mp3,
            StreamFormat::Aac,
            StreamFormat::Opus,
            StreamFormat::Flac,
            StreamFormat::Aac,
            StreamFormat::Opus,
            StreamFormat::Mp3
        ]
    );
}

#[test]
fn the_song_id_reaches_the_converter() {
    let h = converting(None);
    h.get("stream", "id=s-9&format=mp3&maxBitRate=160");
    assert_eq!(
        asked(&h),
        [("s-9".to_owned(), StreamFormat::Mp3, Some(160))]
    );
}

#[test]
fn a_range_request_on_a_conversion_is_ignored() {
    let h = converting(None);
    let reply = h.get_with_header(
        &h.url("stream", "id=s-9&format=mp3"),
        "Range",
        "bytes=100-199",
    );
    assert_converted(&reply, StreamFormat::Mp3, None);
    assert_eq!(reply.header("content-range"), None);
}

#[test]
fn a_song_already_in_the_format_is_converted_only_when_over_the_bitrate() {
    let h = converting(None);
    // s-1 is a 320 kbps mp3, s-10 a 128 kbps one.
    assert_original(&h.get("stream", "id=s-1&format=mp3"));
    assert_original(&h.get("stream", "id=s-1&format=mp3&maxBitRate=320"));
    assert_original(&h.get("stream", "id=s-10&format=mp3&maxBitRate=192"));
    assert!(asked(&h).is_empty());

    assert_converted(
        &h.get("stream", "id=s-1&format=mp3&maxBitRate=128"),
        StreamFormat::Mp3,
        Some(128),
    );
    assert_eq!(
        asked(&h),
        [("s-1".to_owned(), StreamFormat::Mp3, Some(128))]
    );
}

#[test]
fn a_bitrate_alone_converts_what_is_over_it_to_mp3() {
    let h = converting(None);
    assert_converted(
        &h.get("stream", "id=s-1&maxBitRate=128"),
        StreamFormat::Mp3,
        Some(128),
    );
    // A lossless song is over any cap.
    assert_converted(
        &h.get("stream", "id=s-9&maxBitRate=320"),
        StreamFormat::Mp3,
        Some(320),
    );
    // Under the cap, or no cap, it goes as it is.
    assert_original(&h.get("stream", "id=s-10&maxBitRate=256"));
    assert_original(&h.get("stream", "id=s-1&maxBitRate=320"));
    assert_original(&h.get("stream", "id=s-9&maxBitRate=0"));
    assert_eq!(asked(&h).len(), 2);
}

#[test]
fn the_servers_default_cap_applies_when_the_client_names_none() {
    let h = converting(Some(192));
    assert_converted(&h.get("stream", "id=s-1"), StreamFormat::Mp3, Some(192));
    assert_converted(&h.get("stream", "id=s-9"), StreamFormat::Mp3, Some(192));
    assert_original(&h.get("stream", "id=s-10"));
    // The client's own cap replaces it, and so does a client's 0 ("original quality") not.
    assert_converted(
        &h.get("stream", "id=s-1&maxBitRate=96"),
        StreamFormat::Mp3,
        Some(96),
    );
    assert_converted(
        &h.get("stream", "id=s-1&maxBitRate=0"),
        StreamFormat::Mp3,
        Some(192),
    );
    assert_original(&h.get("stream", "id=s-1&maxBitRate=320"));
    // A named format is held to it as well, and raw switches it off.
    assert_converted(
        &h.get("stream", "id=s-1&format=aac"),
        StreamFormat::Aac,
        Some(192),
    );
    assert_original(&h.get("stream", "id=s-1&format=raw"));
    assert_eq!(asked(&h).len(), 5);
}

#[test]
fn a_default_cap_of_zero_is_no_cap() {
    let h = converting(Some(0));
    assert_original(&h.get("stream", "id=s-9"));
    assert!(asked(&h).is_empty());
}

#[test]
fn a_format_the_server_cannot_make_is_ignored() {
    let h = converting(None);
    assert_original(&h.get("stream", "id=s-9&format=wma"));
    assert_converted(
        &h.get("stream", "id=s-9&format=wma&maxBitRate=128"),
        StreamFormat::Mp3,
        Some(128),
    );
    assert_eq!(asked(&h).len(), 1);
}

#[test]
fn a_source_that_cannot_convert_falls_back_to_the_original_with_ranges() {
    let h = Harness::start_converting(Some(128), Transcode::Unsupported);
    for query in ["id=s-9&format=mp3", "id=s-9", "id=s-9&maxBitRate=64"] {
        assert_original(&h.get("stream", query));
    }
    // It was asked each time, and said no.
    assert_eq!(asked(&h).len(), 3);
    let ranged = h.get_with_header(&h.url("stream", "id=s-9&format=mp3"), "Range", "bytes=0-9");
    assert_eq!((ranged.status, ranged.body), (206, file_bytes(0..10)));
}

#[test]
fn a_converter_that_fails_is_reported_as_an_error() {
    let h = Harness::start_converting(None, Transcode::Failing);
    for format in ["", "&f=json"] {
        let reply = h.get("stream", &format!("id=s-9&format=mp3{format}"));
        assert_eq!(reply.status, 200);
        assert_eq!(error_of(&reply), (0, "the encoder is missing".to_owned()));
    }
}

#[test]
fn an_unknown_song_is_404_even_when_a_conversion_is_asked_for() {
    let h = converting(Some(128));
    for query in ["id=nope&format=mp3", "id=nope", "id=nope&maxBitRate=64"] {
        let reply = h.get("stream", query);
        assert_eq!(reply.status, 404, "{query}");
        assert_eq!(error_of(&reply).0, 70, "{query}");
    }
    assert!(asked(&h).is_empty());
}

#[test]
fn a_song_the_library_fails_to_look_up_is_a_generic_error() {
    let h = converting(None);
    let reply = h.get("stream", "id=broken&format=mp3");
    assert_eq!(error_of(&reply), (0, "database is locked".to_owned()));
}

#[test]
fn download_is_never_converted() {
    let h = converting(Some(64));
    let reply = h.get("download", "id=s-9&format=mp3&maxBitRate=32");
    assert_original(&reply);
    assert!(reply.header("content-disposition").is_some());
    assert!(asked(&h).is_empty());
    assert_eq!(h.source.calls.lock().unwrap().open_stream, ["s-9"]);
}

#[test]
fn conversions_work_over_post_and_with_the_view_suffix() {
    let h = converting(None);
    let form = [
        ("u", USER),
        ("p", PASSWORD),
        ("v", "1.16.1"),
        ("c", "t"),
        ("id", "s-9"),
        ("format", "opus"),
        ("maxBitRate", "96"),
    ];
    assert_converted(&h.post("stream", &form), StreamFormat::Opus, Some(96));
    assert_converted(
        &h.get("stream.view", "id=s-9&format=aac"),
        StreamFormat::Aac,
        None,
    );
}

/// Yields `byte` forever and notes when it is dropped, as a converter feeding a stream would.
struct Endless(Arc<AtomicBool>);

impl Read for Endless {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        buf.fill(7);
        Ok(buf.len())
    }
}

impl Drop for Endless {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn a_client_that_hangs_up_stops_the_converter_and_releases_it() {
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = dropped.clone();
    let transcode = Transcode::With(Box::new(move |_, _, _| {
        Some(Ok((
            Box::new(Endless(flag.clone())) as Box<dyn Read + Send>,
            "audio/mpeg",
        )))
    }));
    let h = Harness::start_converting(None, transcode);
    {
        let mut response = agent()
            .get(h.url("stream", "id=s-9&format=mp3"))
            .call()
            .unwrap();
        let mut first = [0u8; 4096];
        response
            .body_mut()
            .as_reader()
            .read_exact(&mut first)
            .unwrap();
        assert_eq!(first, [7; 4096]);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !dropped.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "the converter was never released"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(h.json("ping", "")["status"], "ok");
}

#[test]
fn stopping_the_server_lets_a_conversion_in_flight_finish() {
    let transcode = Transcode::With(Box::new(|_, _, _| {
        Some(Ok((
            Box::new(SlowReader { chunks_left: 5 }) as Box<dyn Read + Send>,
            "audio/mpeg",
        )))
    }));
    let h = Harness::start_converting(None, transcode);
    let (url, agent) = (h.url("stream", "id=s-9&format=mp3"), agent());
    let client = thread::spawn(move || {
        let mut response = agent.get(&url).call().unwrap();
        response.body_mut().read_to_vec().unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.source.calls.lock().unwrap().transcoded.is_empty() {
        assert!(Instant::now() < deadline, "the request never arrived");
        thread::sleep(Duration::from_millis(5));
    }
    h.stop();
    assert_eq!(client.join().unwrap(), vec![b'z'; 5000]);
}
