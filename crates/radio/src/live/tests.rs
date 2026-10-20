use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::mpsc::Receiver,
    thread,
    time::{Duration, Instant},
};

use super::{
    Event, Format, LiveStream, Options, StationInfo, format::samples::*, icy::testing::interleave,
    open_with,
};
use crate::{
    error::{Error, is_cancelled},
    http::{Headers, HttpConfig},
    testing::{Server, head, not_found, ok, respond},
};

const FRAME: usize = 417;

pub(super) fn options() -> Options {
    Options {
        http: HttpConfig {
            stall_timeout: Duration::from_millis(300),
            connect_timeout: Duration::from_secs(2),
            ..HttpConfig::default()
        },
        backoff: Duration::from_millis(5),
        max_backoff: Duration::from_millis(20),
        max_failures: 3,
        hls_poll: Some(Duration::from_millis(20)),
    }
}

/// The start of a SHOUTcast v1 reply, which is not HTTP.
fn icy_head(stream: &mut TcpStream, metaint: usize) {
    let text = format!(
        "ICY 200 OK\r\nicy-notice1:<BR>This stream requires <a href=\"http://www.winamp.com\">Winamp</a><BR>\r\nicy-name:Test FM\r\nicy-genre:Jazz\r\nicy-br:128\r\nicy-url:http://testfm.example.com\r\nicy-description:All test, all day\r\nContent-Type:audio/mpeg\r\nicy-metaint:{metaint}\r\n\r\n"
    );
    let _ = stream.write_all(text.as_bytes());
}

pub(super) fn read_exactly(stream: &mut LiveStream, count: usize) -> Vec<u8> {
    let mut audio = vec![0u8; count];
    stream
        .read_exact(&mut audio)
        .expect("the stream should deliver the audio");
    audio
}

pub(super) fn events(stream: &mut LiveStream) -> Vec<Event> {
    let receiver: Receiver<Event> = stream.take_events().expect("events");
    receiver.try_iter().collect()
}

fn titles(events: &[Event]) -> Vec<(String, u64)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::NowPlaying { track, offset } => Some((track.raw.clone(), *offset)),
            _ => None,
        })
        .collect()
}

#[test]
fn an_icy_stream_yields_clean_audio_titles_and_station_info_at_any_metaint() {
    // 1000 does not divide the frame length, so metadata lands mid-frame.
    for metaint in [1000usize, 4096, 16000] {
        let audio = numbered_mp3(0..100);
        let blocks: [&[u8]; 3] = [
            b"StreamTitle='Miles Davis - So What';StreamUrl='http://x.example.com';",
            b"",
            b"StreamTitle='John Coltrane - Giant Steps';",
        ];
        let body = interleave(&audio, metaint, &blocks);
        let server = Server::start(move |_, stream| {
            icy_head(stream, metaint);
            let _ = stream.write_all(&body);
            thread::sleep(Duration::from_millis(500));
        });
        let mut live = open_with(&server.url("/live"), &options()).unwrap();
        assert_eq!(live.format(), Format::Mp3);
        assert_eq!(
            live.info(),
            &StationInfo {
                name: Some("Test FM".into()),
                bitrate_kbps: Some(128),
                genre: Some("Jazz".into()),
                description: Some("All test, all day".into()),
                url: Some("http://testfm.example.com".into()),
            }
        );
        assert_eq!(
            read_exactly(&mut live, audio.len()),
            audio,
            "metaint {metaint}"
        );
        assert_eq!(server.requests()[0].header("icy-metadata"), Some("1"));

        // A block follows each full interval, so the third (index 2) sits after three of them; one
        // that would come after the end of the audio is never read.
        let expected: Vec<(String, u64)> = [
            ("Miles Davis - So What", 0usize),
            ("John Coltrane - Giant Steps", 2),
        ]
        .into_iter()
        .filter(|&(_, index)| (index + 1) * metaint < audio.len())
        .map(|(title, index)| (title.to_owned(), ((index + 1) * metaint) as u64))
        .collect();
        assert!(!expected.is_empty());
        assert_eq!(titles(&events(&mut live)), expected, "metaint {metaint}");
    }
}

#[test]
fn titles_split_into_artist_and_title_and_decoded_from_latin_1() {
    let audio = numbered_mp3(0..30);
    let body = interleave(
        &audio,
        2000,
        &[b"StreamTitle='Bj\xf6rk - Caf\xe9 \x96 Live';"],
    );
    let server = Server::start(move |_, stream| {
        icy_head(stream, 2000);
        let _ = stream.write_all(&body);
        thread::sleep(Duration::from_millis(500));
    });
    let mut live = open_with(&server.url("/"), &options()).unwrap();
    read_exactly(&mut live, audio.len());
    let Some(Event::NowPlaying { track, .. }) = events(&mut live).into_iter().next() else {
        panic!("no title")
    };
    assert_eq!(track.artist.as_deref(), Some("Bj\u{f6}rk"));
    assert_eq!(track.title.as_deref(), Some("Caf\u{e9} \u{2013} Live"));
}

#[test]
fn an_empty_stream_title_clears_the_current_one() {
    let audio = numbered_mp3(0..30);
    let blocks: [&[u8]; 4] = [
        b"StreamTitle='';",
        b"StreamTitle='A - B';",
        b"StreamTitle='';",
        b"",
    ];
    let body = interleave(&audio, 3000, &blocks);
    let server = Server::start(move |_, stream| {
        icy_head(stream, 3000);
        let _ = stream.write_all(&body);
        thread::sleep(Duration::from_millis(500));
    });
    let mut live = open_with(&server.url("/"), &options()).unwrap();
    read_exactly(&mut live, audio.len());
    assert_eq!(
        titles(&events(&mut live)),
        [("A - B".to_owned(), 6000), (String::new(), 9000)]
    );
}

#[test]
fn the_format_comes_from_the_audio_not_the_content_type() {
    let cases: [(&str, Vec<u8>, Format); 5] = [
        (
            "audio/mpeg",
            [adts_frame(300), adts_frame(300), adts_frame(300)].concat(),
            Format::AacAdts,
        ),
        ("audio/aacp", numbered_mp3(0..4), Format::Mp3),
        (
            "application/octet-stream",
            ogg_page(b"OpusHead\x01\x02"),
            Format::Opus,
        ),
        ("audio/ogg", ogg_page(b"\x01vorbis\x00"), Format::Ogg),
        (
            "audio/flac",
            b"fLaC\0\0\0\"\x10\x00\x10\x00".to_vec(),
            Format::Flac,
        ),
    ];
    for (content_type, body, expected) in cases {
        let server = Server::start(move |_, stream| {
            let _ = head(
                stream,
                "HTTP/1.1 200 OK",
                &[("Content-Type", content_type), ("icy-name", "x")],
            );
            let _ = stream.write_all(&body);
            thread::sleep(Duration::from_millis(300));
        });
        assert_eq!(
            open_with(&server.url("/"), &options()).unwrap().format(),
            expected,
            "{content_type}"
        );
    }
}

#[test]
fn a_content_type_stands_in_when_the_audio_is_unreadable() {
    let server = Server::start(|request, stream| {
        let content_type = if request.path == "/labelled" {
            "audio/mpeg"
        } else {
            "application/octet-stream"
        };
        let _ = head(
            stream,
            "HTTP/1.1 200 OK",
            &[("Content-Type", content_type), ("icy-name", "x")],
        );
        let _ = stream.write_all(&[0x55; 100]);
        thread::sleep(Duration::from_millis(300));
    });
    assert_eq!(
        open_with(&server.url("/labelled"), &options())
            .unwrap()
            .format(),
        Format::Mp3
    );
    assert!(matches!(
        open_with(&server.url("/unlabelled"), &options())
            .err()
            .unwrap(),
        Error::UnknownFormat(_)
    ));
}

#[test]
fn a_dropped_connection_is_resumed_on_a_new_one() {
    let audio = numbered_mp3(0..40);
    let first_part = audio[..FRAME * 15].to_vec();
    let rest = audio[FRAME * 15..].to_vec();
    // The second connection uses a different metaint, as a restarted server might.
    let first = interleave(&first_part, 2000, &[b"StreamTitle='One - First';"]);
    let second = interleave(&rest, 3000, &[b"StreamTitle='Two - Second';"]);
    let server = Server::start(move |request, stream| {
        if request.nth == 1 {
            icy_head(stream, 2000);
            let _ = stream.write_all(&first);
            // Dropping the connection mid-metadata-interval.
        } else {
            icy_head(stream, 3000);
            let _ = stream.write_all(&second);
            thread::sleep(Duration::from_millis(500));
        }
    });
    let mut live = open_with(&server.url("/"), &options()).unwrap();
    // The first connection ends after 6255 audio bytes, of which 3 intervals' worth have metadata
    // blocks: 2000, 4000, 6000.
    assert_eq!(read_exactly(&mut live, audio.len()), audio);
    let seen = events(&mut live);
    assert!(seen.contains(&Event::Reconnecting { attempt: 1 }));
    assert!(seen.contains(&Event::Reconnected));
    assert_eq!(server.requests().len(), 2);
    // Offsets keep counting across the join: the second connection's first title comes after its
    // first 3000 bytes, on top of the 6255 that arrived before.
    assert_eq!(
        titles(&seen),
        [
            ("One - First".to_owned(), 2000),
            ("Two - Second".to_owned(), first_part.len() as u64 + 3000)
        ]
    );
}

#[test]
fn a_connection_that_goes_silent_is_replaced_after_the_stall_timeout() {
    let audio = numbered_mp3(0..20);
    let (first_part, rest) = (audio[..FRAME * 8].to_vec(), audio[FRAME * 8..].to_vec());
    let server = Server::start(move |request, stream| {
        let _ = head(
            stream,
            "HTTP/1.1 200 OK",
            &[("Content-Type", "audio/mpeg"), ("icy-name", "x")],
        );
        if request.nth == 1 {
            let _ = stream.write_all(&first_part);
            thread::sleep(Duration::from_secs(3));
        } else {
            let _ = stream.write_all(&rest);
            thread::sleep(Duration::from_millis(500));
        }
    });
    let mut live = open_with(&server.url("/"), &options()).unwrap();
    let started = Instant::now();
    assert_eq!(read_exactly(&mut live, audio.len()), audio);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert!(events(&mut live).contains(&Event::Reconnected));
}

#[test]
fn reconnecting_gives_up_after_the_configured_number_of_failures() {
    let audio = numbered_mp3(0..6);
    let server = Server::start(move |request, stream| {
        if request.nth == 1 {
            let _ = head(
                stream,
                "HTTP/1.1 200 OK",
                &[("Content-Type", "audio/mpeg"), ("icy-name", "x")],
            );
            let _ = stream.write_all(&audio);
        } else {
            not_found(stream);
        }
    });
    let mut live = open_with(&server.url("/"), &options()).unwrap();
    let mut sink = Vec::new();
    let error = live.read_to_end(&mut sink).unwrap_err();
    assert_eq!(sink.len(), FRAME * 6);
    assert!(error.to_string().contains("404"), "{error}");
    // The original connection and three attempts.
    assert_eq!(server.requests().len(), 4);
    let seen = events(&mut live);
    let attempts: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            Event::Reconnecting { attempt } => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(attempts, [1, 2, 3]);
    assert!(matches!(seen.last(), Some(Event::Failed(reason)) if reason.contains("404")));
}

#[test]
fn failures_only_count_while_no_audio_arrives() {
    // Every connection delivers two frames and drops. With one failure allowed in a row, that is
    // fine for as long as each new connection brings audio.
    let audio = numbered_mp3(0..12);
    let pieces: Vec<Vec<u8>> = audio.chunks(FRAME * 2).map(<[u8]>::to_vec).collect();
    let server = Server::start(move |request, stream| {
        let _ = head(
            stream,
            "HTTP/1.1 200 OK",
            &[("Content-Type", "audio/mpeg"), ("icy-name", "x")],
        );
        if let Some(piece) = pieces.get(request.nth - 1) {
            let _ = stream.write_all(piece);
        } else {
            thread::sleep(Duration::from_millis(500));
        }
    });
    let options = Options {
        max_failures: 1,
        ..options()
    };
    let mut live = open_with(&server.url("/"), &options).unwrap();
    assert_eq!(read_exactly(&mut live, audio.len()), audio);
    assert!(server.requests().len() >= 6);
}

#[test]
fn a_response_with_a_length_is_a_file_and_is_not_reconnected() {
    let audio = numbered_mp3(0..10);
    let served = audio.clone();
    let server = Server::start(move |_, stream| ok(stream, "audio/mpeg", &served));
    let mut live = open_with(&server.url("/song.mp3"), &options()).unwrap();
    let mut got = Vec::new();
    live.read_to_end(&mut got).unwrap();
    assert_eq!(got, audio);
    assert_eq!(server.requests().len(), 1);
    assert!(events(&mut live).is_empty());
}

#[test]
fn cancelling_ends_a_waiting_read_without_reconnecting() {
    let audio = numbered_mp3(0..4);
    let server = Server::start(move |_, stream| {
        let _ = head(
            stream,
            "HTTP/1.1 200 OK",
            &[("Content-Type", "audio/mpeg"), ("icy-name", "x")],
        );
        let _ = stream.write_all(&audio);
        thread::sleep(Duration::from_secs(5));
    });
    let slow = Options {
        http: HttpConfig {
            stall_timeout: Duration::from_secs(30),
            ..options().http
        },
        ..options()
    };
    let mut live = open_with(&server.url("/"), &slow).unwrap();
    read_exactly(&mut live, FRAME * 4);
    let cancel = live.cancel_handle();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        cancel.cancel();
    });
    let started = Instant::now();
    let error = live.read(&mut [0u8; 16]).unwrap_err();
    assert!(is_cancelled(&error), "{error}");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn dropping_the_stream_cancels_it() {
    let server = Server::start(|_, stream| {
        let _ = head(
            stream,
            "HTTP/1.1 200 OK",
            &[("Content-Type", "audio/mpeg"), ("icy-name", "x")],
        );
        let _ = stream.write_all(&numbered_mp3(0..4));
        thread::sleep(Duration::from_millis(300));
    });
    let live = open_with(&server.url("/"), &options()).unwrap();
    let cancel = live.cancel_handle();
    assert!(!cancel.is_cancelled());
    drop(live);
    assert!(cancel.is_cancelled());
}

#[test]
fn events_can_be_taken_once_and_queue_until_then() {
    let audio = numbered_mp3(0..20);
    let body = interleave(&audio, 1500, &[b"StreamTitle='Early - Bird';"]);
    let server = Server::start(move |_, stream| {
        icy_head(stream, 1500);
        let _ = stream.write_all(&body);
        thread::sleep(Duration::from_millis(300));
    });
    let mut live = open_with(&server.url("/"), &options()).unwrap();
    read_exactly(&mut live, audio.len());
    let receiver = live.take_events().unwrap();
    assert!(live.take_events().is_none());
    assert_eq!(receiver.try_iter().count(), 1);
}

#[test]
fn playlists_are_followed_to_the_first_entry_that_connects() {
    let audio = numbered_mp3(0..8);
    let served = audio.clone();
    let server = Server::start(move |request, stream| match request.path.as_str() {
        "/station.pls" => {
            let body = "[playlist]\nNumberOfEntries=3\nFile1=/dead.mp3\nFile2=/page.html\nFile3=/live.mp3\n";
            ok(stream, "audio/x-scpls", body.as_bytes());
        }
        "/page.html" => ok(stream, "text/html", b"<html>hello</html>"),
        "/live.mp3" => {
            let _ = head(
                stream,
                "HTTP/1.1 200 OK",
                &[("Content-Type", "audio/mpeg"), ("icy-name", "Third Time")],
            );
            let _ = stream.write_all(&served);
            thread::sleep(Duration::from_millis(300));
        }
        _ => not_found(stream),
    });
    let mut live = open_with(&server.url("/station.pls"), &options()).unwrap();
    assert_eq!(live.url().path(), "/live.mp3");
    assert_eq!(live.info().name.as_deref(), Some("Third Time"));
    assert_eq!(read_exactly(&mut live, audio.len()), audio);
}

#[test]
fn a_playlist_of_dead_entries_reports_the_last_failure() {
    let server = Server::start(|request, stream| match request.path.as_str() {
        "/station.m3u" => ok(stream, "audio/x-mpegurl", b"#EXTM3U\n/a.mp3\n/b.mp3\n"),
        _ => not_found(stream),
    });
    let error = open_with(&server.url("/station.m3u"), &options())
        .err()
        .unwrap();
    assert!(matches!(error, Error::Http { status: 404, .. }), "{error}");
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn playlists_that_point_at_themselves_do_not_loop_forever() {
    let server = Server::start(|_, stream| ok(stream, "audio/x-mpegurl", b"/loop.m3u\n"));
    let error = open_with(&server.url("/loop.m3u"), &options())
        .err()
        .unwrap();
    assert!(matches!(error, Error::NotAStream(_)), "{error}");
    assert!(server.requests().len() <= 5);
}

#[test]
fn unlabelled_playlists_and_streams_are_told_apart_by_their_bodies() {
    let audio = numbered_mp3(0..6);
    let served = audio.clone();
    let server = Server::start(move |request, stream| match request.path.as_str() {
        "/listing" => respond(
            stream,
            "HTTP/1.1 200 OK",
            "text/plain",
            b"[playlist]\nFile1=/raw\n",
        ),
        _ => {
            let _ = head(
                stream,
                "HTTP/1.1 200 OK",
                &[("Content-Type", "application/octet-stream")],
            );
            let _ = stream.write_all(&served);
            thread::sleep(Duration::from_millis(300));
        }
    });
    let mut live = open_with(&server.url("/listing"), &options()).unwrap();
    assert_eq!(live.url().path(), "/raw");
    assert_eq!(live.format(), Format::Mp3);
    // The bytes peeked at to decide are not lost.
    assert_eq!(read_exactly(&mut live, audio.len()), audio);
}

#[test]
fn html_pages_are_not_streams() {
    let server = Server::start(|_, stream| {
        ok(
            stream,
            "text/html",
            b"<html><body>Welcome to the station</body></html>",
        )
    });
    assert!(matches!(
        open_with(&server.url("/"), &options()).err().unwrap(),
        Error::NotAStream(_)
    ));
}

#[test]
fn bad_addresses_are_rejected_before_any_connection() {
    for bad in [
        "",
        "not a url",
        "ftp://example.com/x",
        "mms://example.com/x",
    ] {
        assert!(
            matches!(
                open_with(bad, &options()).err().unwrap(),
                Error::InvalidUrl(_)
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn reconnect_delays_double_up_to_the_cap() {
    let options = Options {
        backoff: Duration::from_millis(500),
        max_backoff: Duration::from_secs(10),
        ..Options::default()
    };
    let delays: Vec<u128> = (1..=7)
        .map(|attempt| options.delay(attempt).as_millis())
        .collect();
    assert_eq!(delays, [500, 1000, 2000, 4000, 8000, 10_000, 10_000]);
    assert_eq!(options.delay(0).as_millis(), 500);
    assert_eq!(options.delay(u32::MAX).as_millis(), 10_000);
}

#[test]
fn station_info_tolerates_what_servers_send() {
    let info = |pairs: &[(&str, &str)]| StationInfo::from_headers(&Headers::from_pairs(pairs));
    assert_eq!(info(&[]), StationInfo::default());
    assert_eq!(info(&[("icy-br", "128,128")]).bitrate_kbps, Some(128));
    assert_eq!(info(&[("icy-br", " 64 kbps ")]).bitrate_kbps, Some(64));
    assert_eq!(info(&[("icy-br", "fast")]).bitrate_kbps, None);
    assert_eq!(info(&[("ICY-NAME", "  "), ("icy-genre", "Pop")]).name, None);
    assert_eq!(info(&[("Icy-Genre", "Pop")]).genre.as_deref(), Some("Pop"));
}
