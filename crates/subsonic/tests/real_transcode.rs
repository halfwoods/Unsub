//! The real converter behind the server: the playback crate's `transcode_stream` fed a fixture, the
//! way the app's library source does it, and the result decoded again.

mod common;

use std::{
    fs,
    path::{Path, PathBuf},
};

use antistreaming_playback::{Decoder, Target, TranscodeOptions, transcode_stream};
use antistreaming_subsonic::{Error, StreamFormat};
use common::*;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../playback/tests/fixtures/sine.flac")
}

/// How `MusicSource::transcoded` maps onto the converter. Formats without a bitrate in the request
/// get a sensible one.
fn real_converter() -> Transcode {
    Transcode::With(Box::new(|_, format, max_kbps| {
        let target = match format {
            StreamFormat::Mp3 => Target::Mp3 {
                kbps: max_kbps.unwrap_or(192),
            },
            StreamFormat::Aac => Target::Aac {
                kbps: max_kbps.unwrap_or(192),
            },
            StreamFormat::Opus => Target::Opus {
                kbps: max_kbps.unwrap_or(96),
            },
            StreamFormat::Flac => Target::Flac,
        };
        let options = TranscodeOptions {
            target,
            max_sample_rate: None,
            max_bit_depth: None,
            keep_tags: false,
        };
        Some(
            transcode_stream(&fixture(), &options)
                .map_err(|error| Error::Source(error.to_string())),
        )
    }))
}

fn seconds(path: &Path) -> f64 {
    let mut decoder = Decoder::open(path)
        .unwrap_or_else(|error| panic!("{} should decode: {error}", path.display()));
    let mut samples = 0usize;
    while let Some(chunk) = decoder.next_chunk().expect("decoding should not fail") {
        samples += chunk.len();
    }
    samples as f64 / f64::from(decoder.channels()) / f64::from(decoder.sample_rate())
}

#[test]
fn each_format_streams_from_the_real_converter_and_decodes_to_the_same_length() {
    let h = Harness::start_converting(None, real_converter());
    let scratch = tempfile::tempdir().unwrap();
    let original = seconds(&fixture());
    assert!(original > 0.5, "{original}");

    for (format, extension, mime, song) in [
        ("mp3", "mp3", "audio/mpeg", "s-9"),
        ("aac", "aac", "audio/aac", "s-9"),
        ("opus", "opus", "audio/ogg", "s-9"),
        // s-1 is an mp3, so flac is a conversion; s-9 is already flac.
        ("flac", "flac", "audio/flac", "s-1"),
    ] {
        let reply = h.get(
            "stream",
            &format!("id={song}&format={format}&maxBitRate=96"),
        );
        assert_eq!(reply.status, 200, "{format}");
        assert_eq!(reply.header("content-type"), Some(mime), "{format}");
        assert_eq!(
            reply.header("transfer-encoding"),
            Some("chunked"),
            "{format}"
        );
        assert_eq!(reply.header("content-length"), None, "{format}");
        assert!(
            reply.body.len() > 1_000,
            "{format}: {} bytes",
            reply.body.len()
        );

        let path = scratch.path().join(format!("out.{extension}"));
        fs::write(&path, &reply.body).unwrap();
        let converted = seconds(&path);
        assert!(
            (converted - original).abs() < 0.15,
            "{format}: {converted} s against {original} s"
        );
    }
}

#[test]
fn the_bitrate_in_the_request_sets_the_size_of_what_is_streamed() {
    let h = Harness::start_converting(None, real_converter());
    let size = |kbps: u32| {
        h.get("stream", &format!("id=s-9&format=mp3&maxBitRate={kbps}"))
            .body
            .len()
    };
    let (low, high) = (size(64), size(192));
    assert!(high > low * 2, "{high} against {low}");
}

#[test]
fn the_servers_default_cap_converts_a_lossless_song_for_a_client_that_names_nothing() {
    let h = Harness::start_converting(Some(96), real_converter());
    let reply = h.get("stream", "id=s-9");
    assert_eq!(reply.header("content-type"), Some("audio/mpeg"));
    assert_eq!(reply.header("transfer-encoding"), Some("chunked"));
    assert!(reply.body.len() > 1_000);
    // raw is still the stored file, with ranges.
    let raw = h.get("stream", "id=s-9&format=raw");
    assert_eq!(raw.header("accept-ranges"), Some("bytes"));
    assert_eq!(raw.body, file_bytes(0..SONG_FILE_LEN));
}

#[test]
fn a_song_the_converter_cannot_open_is_an_error_not_a_hang() {
    let h = Harness::start_converting(
        None,
        Transcode::With(Box::new(|_, _, _| {
            let options = TranscodeOptions {
                target: Target::Mp3 { kbps: 128 },
                max_sample_rate: None,
                max_bit_depth: None,
                keep_tags: false,
            };
            let not_audio = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../playback/tests/fixtures/not-audio.txt");
            Some(
                transcode_stream(&not_audio, &options)
                    .map_err(|error| Error::Source(error.to_string())),
            )
        })),
    );
    let reply = h.get("stream", "id=s-9&format=mp3");
    assert_eq!(reply.status, 200);
    assert_eq!(error_of(&reply).0, 0);
}
