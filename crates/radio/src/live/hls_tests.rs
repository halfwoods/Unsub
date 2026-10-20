//! HLS against a fake origin whose playlist changes with every request, as a live one does.

use std::{
    io::Read,
    thread,
    time::{Duration, Instant},
};

use aes::Aes128;
use cbc::{
    Encryptor,
    cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7},
};

use super::{
    Event, Format, Options,
    format::samples::{adts_frame, id3v2, ts_packets},
    open_with,
    tests::{events, options, read_exactly},
};
use crate::{
    error::{Error, is_cancelled},
    testing::{Server, not_found, ok},
};

/// A transport-stream segment whose packets carry its number, so the output says which came through.
fn ts_segment(number: u64) -> Vec<u8> {
    let mut segment = ts_packets(3);
    for packet in segment.chunks_mut(188) {
        packet[1] = u8::try_from(number).unwrap();
    }
    segment
}

/// A media playlist listing segments `first..first + count`.
fn playlist(first: u64, count: u64, ended: bool) -> String {
    let mut text = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:{first}\n"
    );
    for number in first..first + count {
        text.push_str(&format!("#EXTINF:6.0,\nseg/{number}.ts\n"));
    }
    if ended {
        text.push_str("#EXT-X-ENDLIST\n");
    }
    text
}

const PLAYLIST: &str = "/hls/live.m3u8";

/// Serves `playlist(n)` for the nth fetch of the playlist and `segment(number)` for `/hls/seg/N.ts`.
fn origin(
    playlist: impl Fn(usize) -> String + Send + Sync + 'static,
    segment: impl Fn(u64) -> Option<Vec<u8>> + Send + Sync + 'static,
) -> Server {
    Server::start(move |request, stream| {
        if request.path == PLAYLIST {
            ok(
                stream,
                "application/vnd.apple.mpegurl",
                playlist(request.nth).as_bytes(),
            );
            return;
        }
        let number = request
            .path
            .strip_prefix("/hls/seg/")
            .and_then(|rest| rest.strip_suffix(".ts"))
            .and_then(|n| n.parse().ok());
        match number.and_then(&segment) {
            Some(bytes) => ok(stream, "video/mp2t", &bytes),
            None => not_found(stream),
        }
    })
}

fn read_all(stream: &mut super::LiveStream) -> Vec<u8> {
    let mut audio = Vec::new();
    stream.read_to_end(&mut audio).unwrap();
    audio
}

/// Which segments a transport-stream body is made of, in order. Comparing these rather than the
/// bytes keeps a failure readable.
fn numbers(audio: &[u8]) -> Vec<u8> {
    assert_eq!(
        audio.len() % SEGMENT_BYTES,
        0,
        "a partial segment came through"
    );
    audio
        .chunks(SEGMENT_BYTES)
        .map(|segment| segment[1])
        .collect()
}

const SEGMENT_BYTES: usize = 3 * 188;

#[test]
fn a_rolling_live_playlist_is_joined_back_to_back_from_three_segments_behind_the_edge() {
    // Five segments listed, then the window slides one at a time until the stream ends.
    let server = origin(
        |nth| {
            let first = nth as u64 - 1;
            playlist(first, 5, first == 3)
        },
        |number| Some(ts_segment(number)),
    );
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    assert_eq!(live.format(), Format::MpegTs);
    // Starts at segment 2 (three behind the end of 0..5), then 5, 6, 7 arrive as the window moves.
    assert_eq!(numbers(&read_all(&mut live)), [2, 3, 4, 5, 6, 7]);
    assert_eq!(
        server.hits("/hls/seg/0.ts") + server.hits("/hls/seg/1.ts"),
        0
    );
    assert!(events(&mut live).is_empty());
}

#[test]
fn segments_that_fell_off_the_playlist_are_reported_as_skipped() {
    let server = origin(
        |nth| match nth {
            1 => playlist(0, 4, false),
            // The player was away: 4 to 7 have rolled off.
            2 => playlist(8, 4, false),
            _ => playlist(8, 4, true),
        },
        |number| Some(ts_segment(number)),
    );
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    assert_eq!(numbers(&read_all(&mut live)), [1, 2, 3, 8, 9, 10, 11]);
    assert_eq!(events(&mut live), [Event::Skipped { segments: Some(4) }]);
}

#[test]
fn a_server_that_restarts_its_numbering_is_followed_from_its_new_live_edge() {
    let server = origin(
        |nth| match nth {
            1 => playlist(100, 4, false),
            2 => playlist(0, 4, false),
            _ => playlist(0, 4, true),
        },
        |number| Some(ts_segment(number)),
    );
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    assert_eq!(numbers(&read_all(&mut live)), [101, 102, 103, 1, 2, 3]);
    assert_eq!(events(&mut live), [Event::Skipped { segments: None }]);
}

#[test]
fn a_stale_playlist_from_a_lagging_cache_is_ignored() {
    let server = origin(
        |nth| match nth {
            1 => playlist(0, 5, false),
            // An older copy of the same playlist, as a CDN edge that has not caught up serves.
            2 => playlist(0, 4, false),
            _ => playlist(1, 5, true),
        },
        |number| Some(ts_segment(number)),
    );
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    assert_eq!(numbers(&read_all(&mut live)), [2, 3, 4, 5]);
    assert!(events(&mut live).is_empty());
}

#[test]
fn a_finished_playlist_plays_from_its_first_segment() {
    let server = origin(|_| playlist(0, 5, true), |number| Some(ts_segment(number)));
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    assert_eq!(numbers(&read_all(&mut live)), [0, 1, 2, 3, 4]);
    assert_eq!(server.hits(PLAYLIST), 1);
}

#[test]
fn the_poll_interval_override_sets_the_pace_of_playlist_reloads() {
    let server = origin(
        |nth| {
            if nth < 3 {
                playlist(0, 2, false)
            } else {
                playlist(0, 2, true)
            }
        },
        |number| Some(ts_segment(number)),
    );
    // The playlist's own target duration is six seconds; the 20 ms override is what keeps this fast.
    let started = Instant::now();
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    read_all(&mut live);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(server.hits(PLAYLIST), 3);
}

#[test]
fn a_master_playlist_leads_to_the_audio_only_variant_under_320_kbps() {
    let server = Server::start(|request, stream| match request.path.as_str() {
        "/hls/master.m3u8" => {
            let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=64000,CODECS=\"mp4a.40.5\"\nlow/live.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=256000,CODECS=\"mp4a.40.2\"\nhigh/live.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=700000,CODECS=\"avc1.42001e,mp4a.40.2\",RESOLUTION=640x360\nvideo/live.m3u8\n";
            ok(stream, "application/x-mpegURL", text.as_bytes());
        }
        "/hls/high/live.m3u8" => ok(
            stream,
            "application/x-mpegURL",
            playlist(0, 3, true).replace("seg/", "").as_bytes(),
        ),
        path if path.starts_with("/hls/high/") => ok(stream, "video/mp2t", &ts_segment(7)),
        _ => not_found(stream),
    });
    let mut live = open_with(&server.url("/hls/master.m3u8"), &options()).unwrap();
    assert_eq!(live.url().path(), "/hls/high/live.m3u8");
    assert_eq!(numbers(&read_all(&mut live)), [7, 7, 7]);
    assert_eq!(
        server.hits("/hls/low/live.m3u8") + server.hits("/hls/video/live.m3u8"),
        0
    );
}

fn encrypt(data: &[u8], key: &[u8; 16], iv: [u8; 16]) -> Vec<u8> {
    Encryptor::<Aes128>::new(&(*key).into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(data)
}

#[test]
fn aes_128_segments_are_decrypted_with_the_sequence_number_as_the_default_iv() {
    let key = *b"0123456789abcdef";
    let server = Server::start(move |request, stream| match request.path.as_str() {
        PLAYLIST => {
            let text = playlist(5, 3, true).replace(
                "#EXT-X-VERSION:3\n",
                "#EXT-X-VERSION:3\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n",
            );
            ok(stream, "application/vnd.apple.mpegurl", text.as_bytes());
        }
        "/hls/key.bin" => ok(stream, "application/octet-stream", &key),
        path => match path
            .strip_prefix("/hls/seg/")
            .and_then(|rest| rest.strip_suffix(".ts"))
            .and_then(|n| n.parse::<u64>().ok())
        {
            Some(number) => ok(
                stream,
                "video/mp2t",
                &encrypt(&ts_segment(number), &key, u128::from(number).to_be_bytes()),
            ),
            None => not_found(stream),
        },
    });
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    assert_eq!(live.format(), Format::MpegTs);
    assert_eq!(numbers(&read_all(&mut live)), [5, 6, 7]);
    assert_eq!(server.hits("/hls/key.bin"), 1);
}

#[test]
fn an_explicit_iv_is_used_when_the_playlist_gives_one() {
    let key = *b"fedcba9876543210";
    let iv = *b"\x0f\x0e\x0d\x0c\x0b\x0a\x09\x08\x07\x06\x05\x04\x03\x02\x01\x00";
    let server = Server::start(move |request, stream| match request.path.as_str() {
        PLAYLIST => {
            let text = playlist(0, 2, true).replace("#EXT-X-VERSION:3\n", "#EXT-X-VERSION:3\n#EXT-X-KEY:METHOD=AES-128,URI=\"/hls/key.bin\",IV=0x0f0e0d0c0b0a09080706050403020100\n");
            ok(stream, "application/vnd.apple.mpegurl", text.as_bytes());
        }
        "/hls/key.bin" => ok(stream, "application/octet-stream", &key),
        _ => ok(stream, "video/mp2t", &encrypt(&ts_segment(9), &key, iv)),
    });
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    assert_eq!(numbers(&read_all(&mut live)), [9, 9]);
}

#[test]
fn a_wrong_key_is_an_error_not_noise() {
    let server = Server::start(|request, stream| match request.path.as_str() {
        PLAYLIST => {
            let text = playlist(0, 2, true).replace(
                "#EXT-X-VERSION:3\n",
                "#EXT-X-VERSION:3\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n",
            );
            ok(stream, "application/vnd.apple.mpegurl", text.as_bytes());
        }
        "/hls/key.bin" => ok(
            stream,
            "application/octet-stream",
            b"not sixteen bytes long",
        ),
        _ => ok(stream, "video/mp2t", &ts_segment(1)),
    });
    assert!(matches!(
        open_with(
            &server.url(PLAYLIST),
            &Options {
                max_failures: 1,
                ..options()
            }
        )
        .err()
        .unwrap(),
        Error::Hls(_)
    ));
}

#[test]
fn sample_aes_and_byte_ranges_are_unsupported() {
    for (tag, expected) in [
        ("#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"k\"\n", "SAMPLE-AES"),
        ("#EXT-X-BYTERANGE:100@0\n", "byte-range"),
    ] {
        let text = playlist(0, 2, true).replace(
            "#EXTINF:6.0,\nseg/0.ts",
            &format!("#EXTINF:6.0,\n{tag}seg/0.ts"),
        );
        let server = Server::start(move |_, stream| {
            ok(stream, "application/vnd.apple.mpegurl", text.as_bytes())
        });
        let error = open_with(&server.url(PLAYLIST), &options()).err().unwrap();
        assert!(
            matches!(&error, Error::Unsupported(what) if what.contains(expected)),
            "{error}"
        );
    }
}

#[test]
fn fragmented_mp4_is_delivered_with_its_init_segment_first_and_once() {
    let init = b"\0\0\0\x18ftypiso5\0\0\0\0iso5dash".to_vec();
    let fragment = |number: u8| [b"\0\0\0\x10moof\0\0\0\0\0\0\0".as_slice(), &[number]].concat();
    let (served_init, served_fragment) = (init.clone(), fragment);
    let server = Server::start(move |request, stream| match request.path.as_str() {
        PLAYLIST => {
            let text = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\nf0.m4s\n#EXTINF:4,\nf1.m4s\n#EXTINF:4,\nf2.m4s\n#EXT-X-ENDLIST\n";
            ok(stream, "application/vnd.apple.mpegurl", text.as_bytes());
        }
        "/hls/init.mp4" => ok(stream, "video/mp4", &served_init),
        path => match path
            .strip_prefix("/hls/f")
            .and_then(|rest| rest.strip_suffix(".m4s"))
            .and_then(|n| n.parse::<u8>().ok())
        {
            Some(number) => ok(stream, "video/iso.segment", &served_fragment(number)),
            None => not_found(stream),
        },
    });
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    assert_eq!(live.format(), Format::Fmp4);
    let expected: Vec<u8> = [
        init.clone(),
        (0..3)
            .flat_map(|n| [b"\0\0\0\x10moof\0\0\0\0\0\0\0".as_slice(), &[n]].concat())
            .collect(),
    ]
    .concat();
    assert!(
        read_all(&mut live) == expected,
        "init segment first, then the three fragments"
    );
    assert_eq!(server.hits("/hls/init.mp4"), 1);
}

#[test]
fn packed_audio_segments_lose_their_timestamp_tags_and_report_their_codec() {
    let frames = [adts_frame(300), adts_frame(300), adts_frame(300)].concat();
    let served = [id3v2(45), frames.clone()].concat();
    let server = Server::start(move |request, stream| {
        if request.path == PLAYLIST {
            ok(
                stream,
                "application/vnd.apple.mpegurl",
                playlist(0, 3, true).replace(".ts", ".aac").as_bytes(),
            );
        } else {
            ok(stream, "audio/aac", &served);
        }
    });
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    assert_eq!(live.format(), Format::AacAdts);
    assert!(
        read_all(&mut live) == frames.repeat(3),
        "the output is not three untagged copies"
    );
}

#[test]
fn a_segment_that_fails_once_is_retried_and_the_stream_carries_on() {
    let server = Server::start(|request, stream| {
        if request.path == PLAYLIST {
            ok(
                stream,
                "application/vnd.apple.mpegurl",
                playlist(0, 4, true).as_bytes(),
            );
        } else if request.path == "/hls/seg/2.ts" && request.nth == 1 {
            not_found(stream);
        } else {
            let number = request
                .path
                .trim_start_matches("/hls/seg/")
                .trim_end_matches(".ts")
                .parse()
                .unwrap_or(0);
            ok(stream, "video/mp2t", &ts_segment(number));
        }
    });
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    assert_eq!(numbers(&read_all(&mut live)), [0, 1, 2, 3]);
    assert_eq!(
        events(&mut live),
        [Event::Reconnecting { attempt: 1 }, Event::Reconnected]
    );
}

#[test]
fn a_segment_that_never_comes_ends_the_stream_with_an_error_after_the_allowed_failures() {
    let server = origin(
        |_| playlist(0, 4, false),
        |number| (number != 2).then(|| ts_segment(number)),
    );
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    let mut got = Vec::new();
    let error = live.read_to_end(&mut got).unwrap_err();
    assert!(error.to_string().contains("404"), "{error}");
    assert_eq!(numbers(&got), [1]);
    assert_eq!(server.hits("/hls/seg/2.ts"), 4);
    assert!(
        matches!(events(&mut live).last(), Some(Event::Failed(reason)) if reason.contains("404"))
    );
}

#[test]
fn a_playlist_that_stops_answering_is_retried_and_then_given_up_on() {
    let server = Server::start(|request, stream| {
        if request.path == PLAYLIST && request.nth <= 2 {
            ok(
                stream,
                "application/vnd.apple.mpegurl",
                playlist(0, 2, false).as_bytes(),
            );
        } else if request.path == PLAYLIST {
            not_found(stream);
        } else {
            ok(stream, "video/mp2t", &ts_segment(0));
        }
    });
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    let mut got = Vec::new();
    assert!(live.read_to_end(&mut got).is_err());
    // The first playlist's two segments, then nothing new to fetch.
    assert_eq!(numbers(&got), [0, 0]);
}

#[test]
fn cancelling_stops_a_live_playlist_that_would_never_end() {
    let server = origin(
        |nth| playlist(nth as u64, 4, false),
        |number| Some(ts_segment(number)),
    );
    let mut live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    read_exactly(&mut live, 188 * 3);
    live.cancel_handle().cancel();
    let started = Instant::now();
    let mut sink = Vec::new();
    // What was already fetched may still come out; then the cancellation does.
    let error = live.read_to_end(&mut sink).unwrap_err();
    assert!(is_cancelled(&error), "{error}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn dropping_a_live_hls_stream_stops_its_fetching() {
    let server = origin(
        |nth| playlist(nth as u64, 4, false),
        |number| Some(ts_segment(number)),
    );
    let live = open_with(&server.url(PLAYLIST), &options()).unwrap();
    drop(live);
    thread::sleep(Duration::from_millis(300));
    let settled = server.requests().len();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(server.requests().len(), settled);
}

#[test]
fn an_hls_playlist_reached_through_a_pls_or_unlabelled_is_still_recognised() {
    let server = Server::start(|request, stream| match request.path.as_str() {
        "/station.pls" => ok(
            stream,
            "audio/x-scpls",
            b"[playlist]\nFile1=/hls/live.m3u8\n",
        ),
        PLAYLIST => ok(stream, "text/plain", playlist(0, 2, true).as_bytes()),
        _ => ok(stream, "video/mp2t", &ts_segment(4)),
    });
    let mut live = open_with(&server.url("/station.pls"), &options()).unwrap();
    assert_eq!(live.url().path(), PLAYLIST);
    assert_eq!(numbers(&read_all(&mut live)), [4, 4]);
}
