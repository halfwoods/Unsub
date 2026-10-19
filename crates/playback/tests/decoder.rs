use std::path::PathBuf;

use antistreaming_playback::{Decoder, Error};

struct Fixture {
    file: &'static str,
    sample_rate: u32,
    channels: u16,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        file: "sine.flac",
        sample_rate: 44_100,
        channels: 2,
    },
    Fixture {
        file: "sine-alac.m4a",
        sample_rate: 48_000,
        channels: 2,
    },
    Fixture {
        file: "sine-aac.m4a",
        sample_rate: 44_100,
        channels: 2,
    },
    Fixture {
        file: "sine.mp3",
        sample_rate: 44_100,
        channels: 2,
    },
    Fixture {
        file: "sine.ogg",
        sample_rate: 44_100,
        channels: 2,
    },
    Fixture {
        file: "sine-mono.wav",
        sample_rate: 44_100,
        channels: 1,
    },
    Fixture {
        file: "sine-22k.aiff",
        sample_rate: 22_050,
        channels: 2,
    },
];

fn fixture_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(file)
}

fn open(fixture: &Fixture) -> Decoder {
    Decoder::open(&fixture_path(fixture.file)).unwrap_or_else(|error| {
        panic!("{} should open: {error}", fixture.file);
    })
}

fn decode_to_end(decoder: &mut Decoder) -> Vec<f32> {
    let mut samples = Vec::new();
    while let Some(chunk) = decoder.next_chunk().expect("decoding should not fail") {
        samples.extend_from_slice(chunk);
    }
    samples
}

/// Rising zero crossings per second of the first channel, measured over the middle half so that
/// encoder delay and padding at the edges do not count.
fn estimated_hz(samples: &[f32], fixture: &Fixture) -> f32 {
    let first_channel: Vec<f32> = samples
        .iter()
        .step_by(usize::from(fixture.channels))
        .copied()
        .collect();
    let middle = &first_channel[first_channel.len() / 4..first_channel.len() * 3 / 4];
    let rising = middle
        .windows(2)
        .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
        .count();
    rising as f32 * fixture.sample_rate as f32 / middle.len() as f32
}

fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0, |peak, sample| peak.max(sample.abs()))
}

#[test]
fn every_format_opens_and_reports_its_rate_and_channels() {
    for fixture in FIXTURES {
        let decoder = open(fixture);
        assert_eq!(
            decoder.sample_rate(),
            fixture.sample_rate,
            "{}",
            fixture.file
        );
        assert_eq!(decoder.channels(), fixture.channels, "{}", fixture.file);
    }
}

#[test]
fn reported_duration_is_about_one_second() {
    for fixture in FIXTURES {
        if let Some(duration_ms) = open(fixture).duration_ms() {
            assert!(
                duration_ms.abs_diff(1_000) <= 50,
                "{}: {duration_ms} ms",
                fixture.file
            );
        }
    }
}

#[test]
fn decoded_length_is_within_two_percent_of_one_second() {
    for fixture in FIXTURES {
        let samples = decode_to_end(&mut open(fixture));
        let frames = samples.len() / usize::from(fixture.channels);
        let expected = fixture.sample_rate as usize;
        assert!(
            frames.abs_diff(expected) <= expected / 50,
            "{}: {frames} frames, expected about {expected}",
            fixture.file
        );
    }
}

#[test]
fn decoded_audio_is_the_440_hz_sine() {
    for fixture in FIXTURES {
        let samples = decode_to_end(&mut open(fixture));
        let level = peak(&samples);
        assert!(
            (0.4..=0.8).contains(&level),
            "{}: peak {level}",
            fixture.file
        );
        let hz = estimated_hz(&samples, fixture);
        assert!((hz - 440.0).abs() < 15.0, "{}: {hz} Hz", fixture.file);
    }
}

#[test]
fn seek_to_half_a_second_lands_on_that_frame_and_the_rest_still_decodes() {
    for fixture in FIXTURES {
        let mut decoder = open(fixture);
        let total = decode_to_end(&mut open(fixture)).len() / usize::from(fixture.channels);

        let landed = decoder.seek(500).expect("seek should succeed");
        let remaining_samples = decode_to_end(&mut decoder);
        let remaining = remaining_samples.len() / usize::from(fixture.channels);

        assert_eq!(landed, 500, "{}", fixture.file);
        assert_eq!(
            remaining,
            total - fixture.sample_rate as usize / 2,
            "{}",
            fixture.file
        );
        assert!(
            peak(&remaining_samples) > 0.4,
            "{}: silent after seek",
            fixture.file
        );
    }
}

#[test]
fn seek_after_reaching_the_end_replays_from_the_new_position() {
    for fixture in FIXTURES {
        let mut decoder = open(fixture);
        let total = decode_to_end(&mut decoder).len();

        decoder.seek(0).expect("seek to start should succeed");
        let replay = decode_to_end(&mut decoder).len();

        let tolerance = usize::from(fixture.channels) * fixture.sample_rate as usize / 50;
        assert!(
            replay.abs_diff(total) <= tolerance,
            "{}: {replay} vs {total}",
            fixture.file
        );
    }
}

#[test]
fn consecutive_seeks_are_independent() {
    let fixture = &FIXTURES[0];
    let mut decoder = open(fixture);
    decoder.seek(800).unwrap();
    decoder.next_chunk().unwrap();
    decoder.seek(100).unwrap();
    let frames = decode_to_end(&mut decoder).len() / usize::from(fixture.channels);
    assert_eq!(frames, fixture.sample_rate as usize * 9 / 10);
}

#[test]
fn a_non_audio_file_is_an_error() {
    let result = Decoder::open(&fixture_path("not-audio.txt"));
    assert!(
        matches!(result, Err(Error::Decode(_))),
        "{:?}",
        result.err()
    );
}

#[test]
fn a_missing_file_is_an_io_error() {
    let result = Decoder::open(&fixture_path("does-not-exist.flac"));
    assert!(matches!(result, Err(Error::Io(_))), "{:?}", result.err());
}

#[test]
fn a_file_cut_short_plays_what_is_there_and_ends_cleanly() {
    let bytes = std::fs::read(fixture_path("sine.flac")).unwrap();
    let path = std::env::temp_dir().join("antistreaming-truncated.flac");
    std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();

    let frames = decode_to_end(&mut Decoder::open(&path).unwrap()).len() / 2;
    std::fs::remove_file(&path).unwrap();

    assert!((1..44_100).contains(&frames), "{frames} frames");
}

#[test]
fn a_file_with_headers_but_no_decodable_audio_is_an_error() {
    let bytes = std::fs::read(fixture_path("sine.flac")).unwrap();
    let path = std::env::temp_dir().join("antistreaming-garbage.flac");
    // Keep the metadata blocks, replace the audio frames with noise.
    let audio_start = bytes
        .windows(2)
        .position(|pair| pair == [0xFF, 0xF8])
        .unwrap();
    let mut damaged = bytes[..audio_start].to_vec();
    damaged.extend((0..4_000u32).map(|n| (n.wrapping_mul(2_654_435_761) >> 24) as u8 | 1));
    std::fs::write(&path, damaged).unwrap();

    let outcome = Decoder::open(&path).and_then(|mut decoder| {
        while decoder.next_chunk()?.is_some() {}
        Ok(())
    });
    std::fs::remove_file(&path).unwrap();

    assert!(
        outcome.is_err(),
        "garbage audio must not look like a finished track"
    );
}

#[test]
fn bit_depth_is_reported_for_lossless_formats_and_absent_for_lossy_ones() {
    let depth = |file: &'static str| {
        let fixture = Fixture {
            file,
            sample_rate: 0,
            channels: 0,
        };
        open(&fixture).bit_depth()
    };
    for (file, bits) in [
        ("sine.flac", 16),
        ("sine-24bit.flac", 24),
        ("sine-alac.m4a", 16),
        ("sine-mono.wav", 16),
        ("sine-22k.aiff", 16),
    ] {
        assert_eq!(depth(file), Some(bits), "{file}");
    }
    for file in ["sine.mp3", "sine-aac.m4a", "sine.ogg"] {
        assert_eq!(depth(file), None, "{file}");
    }
}
