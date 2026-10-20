//! Formats symphonia cannot read (Opus, WavPack, Monkey's Audio, DSD) go through the FFmpeg
//! fallback; everything else must keep using symphonia.

use std::path::PathBuf;

use unsub_playback::{Backend, Decoder};

fn open(file: &str) -> Decoder {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(file);
    Decoder::open(&path).unwrap_or_else(|error| panic!("{file} should open: {error}"))
}

const SYMPHONIA_FIXTURES: &[&str] = &[
    "sine.flac",
    "sine-alac.m4a",
    "sine-aac.m4a",
    "sine.mp3",
    "sine.ogg",
    "sine-mono.wav",
    "sine-22k.aiff",
];

#[test]
fn symphonia_keeps_every_format_it_can_read() {
    for file in SYMPHONIA_FIXTURES {
        assert_eq!(open(file).backend(), Backend::Symphonia, "{file}");
    }
}

#[cfg(feature = "ffmpeg")]
mod ffmpeg {
    use unsub_playback::ChannelLayout;

    use super::*;
    fn decode_to_end(decoder: &mut Decoder) -> Vec<f32> {
        let mut samples = Vec::new();
        while let Some(chunk) = decoder.next_chunk().expect("decoding should not fail") {
            samples.extend_from_slice(chunk);
        }
        samples
    }

    fn channel(samples: &[f32], channels: usize, index: usize) -> Vec<f32> {
        samples
            .iter()
            .skip(index)
            .step_by(channels)
            .copied()
            .collect()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples
            .iter()
            .fold(0.0, |peak, sample| peak.max(sample.abs()))
    }

    /// Rising zero crossings per second over the middle half, after averaging `smooth` samples so
    /// DSD's ultrasonic noise does not add crossings of its own.
    fn estimated_hz(samples: &[f32], rate: u32, smooth: usize) -> f32 {
        let smoothed: Vec<f32> = samples
            .chunks_exact(smooth)
            .map(|chunk| chunk.iter().sum::<f32>() / smooth as f32)
            .collect();
        let middle = &smoothed[smoothed.len() / 4..smoothed.len() * 3 / 4];
        let rising = middle
            .windows(2)
            .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
            .count();
        rising as f32 * (rate as f32 / smooth as f32) / middle.len() as f32
    }

    struct Fixture {
        file: &'static str,
        sample_rate: u32,
        /// Frames the file holds, which every one of these lossless-or-trimmed formats must return.
        frames: usize,
        bit_depth: Option<u32>,
    }

    const FFMPEG_FIXTURES: &[Fixture] = &[
        Fixture {
            file: "sine.opus",
            sample_rate: 48_000,
            frames: 24_000,
            bit_depth: None,
        },
        Fixture {
            file: "sine.wv",
            sample_rate: 44_100,
            frames: 22_050,
            bit_depth: Some(16),
        },
        Fixture {
            file: "sine.ape",
            sample_rate: 44_100,
            frames: 22_050,
            bit_depth: Some(16),
        },
    ];

    #[test]
    fn unreadable_formats_fall_back_to_ffmpeg() {
        for file in FFMPEG_FIXTURES
            .iter()
            .map(|f| f.file)
            .chain(["sine-dsd64.dsf", "sine-dsd64.dff"])
        {
            assert_eq!(open(file).backend(), Backend::Ffmpeg, "{file}");
        }
    }

    #[test]
    fn reports_rate_channels_depth_and_length() {
        for fixture in FFMPEG_FIXTURES {
            let decoder = open(fixture.file);
            assert_eq!(
                decoder.sample_rate(),
                fixture.sample_rate,
                "{}",
                fixture.file
            );
            assert_eq!(decoder.channels(), 2, "{}", fixture.file);
            assert_eq!(
                decoder.channel_layout(),
                ChannelLayout::Stereo,
                "{}",
                fixture.file
            );
            assert_eq!(decoder.bit_depth(), fixture.bit_depth, "{}", fixture.file);
            let duration = decoder.duration_ms().expect("duration known");
            assert!(
                duration.abs_diff(500) <= 30,
                "{}: {duration} ms",
                fixture.file
            );
        }
    }

    #[test]
    fn decodes_the_exact_number_of_frames() {
        for fixture in FFMPEG_FIXTURES {
            let frames = decode_to_end(&mut open(fixture.file)).len() / 2;
            assert_eq!(frames, fixture.frames, "{}", fixture.file);
        }
    }

    #[test]
    fn decodes_a_440_hz_sine_at_the_right_level() {
        for fixture in FFMPEG_FIXTURES {
            let samples = decode_to_end(&mut open(fixture.file));
            let left = channel(&samples, 2, 0);
            let level = peak(&left);
            assert!(
                (0.45..=0.6).contains(&level),
                "{}: peak {level}",
                fixture.file
            );
            let hz = estimated_hz(&left, fixture.sample_rate, 1);
            assert!((hz - 440.0).abs() < 15.0, "{}: {hz} Hz", fixture.file);
        }
    }

    #[test]
    fn wavpack_and_monkeys_audio_are_lossless_and_agree_sample_for_sample() {
        let wavpack = decode_to_end(&mut open("sine.wv"));
        let ape = decode_to_end(&mut open("sine.ape"));
        assert!(wavpack == ape, "both lossless files carry the same PCM");

        // And that PCM is the sine the source was made of, to within 16-bit rounding.
        let worst = channel(&wavpack, 2, 0)
            .iter()
            .enumerate()
            .map(|(n, sample)| {
                let ideal = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 44_100.0).sin();
                (sample - ideal).abs()
            })
            .fold(0.0, f32::max);
        assert!(worst < 3.0 / 32_768.0, "worst deviation {worst}");
    }

    #[test]
    fn seeks_land_on_the_requested_frame() {
        for fixture in FFMPEG_FIXTURES {
            let mut decoder = open(fixture.file);
            let landed = decoder.seek(250).expect("seek should succeed");
            let remaining = decode_to_end(&mut decoder).len() / 2;
            assert_eq!(landed, 250, "{}", fixture.file);
            assert_eq!(
                remaining,
                fixture.frames - fixture.sample_rate as usize / 4,
                "{}",
                fixture.file
            );
        }
    }

    #[test]
    fn seeking_after_reading_to_the_end_restarts_cleanly() {
        for fixture in FFMPEG_FIXTURES {
            let mut decoder = open(fixture.file);
            decode_to_end(&mut decoder);
            decoder.seek(0).expect("seek to start should succeed");
            let again = decode_to_end(&mut decoder).len() / 2;
            assert_eq!(again, fixture.frames, "{}", fixture.file);
        }
    }

    #[test]
    fn dsd_is_converted_to_pcm_at_an_eighth_of_the_bit_rate() {
        for file in ["sine-dsd64.dsf", "sine-dsd64.dff"] {
            let mut decoder = open(file);
            assert_eq!(decoder.sample_rate(), 352_800, "{file}");
            assert_eq!(decoder.channels(), 2, "{file}");
            assert_eq!(decoder.bit_depth(), None, "{file}");

            let samples = decode_to_end(&mut decoder);
            assert_eq!(samples.len() / 2, 17_640, "{file}: 50 ms");
            let left = channel(&samples, 2, 0);
            let right = channel(&samples, 2, 1);
            // Left was modulated at twice the amplitude of right.
            let (left_level, right_level) = (peak(&left[200..]), peak(&right[200..]));
            assert!(
                left_level > 1.6 * right_level && (0.4..0.65).contains(&left_level),
                "{file}: left {left_level}, right {right_level}"
            );
            let hz = estimated_hz(&left, 352_800, 16);
            assert!((hz - 440.0).abs() < 40.0, "{file}: {hz} Hz");
        }
    }

    #[test]
    fn dsd_seeks_by_time_like_everything_else() {
        let mut decoder = open("sine-dsd64.dsf");
        assert_eq!(decoder.seek(25).unwrap(), 25);
        let remaining = decode_to_end(&mut decoder).len() / 2;
        assert_eq!(remaining, 17_640 - 8_820);
    }

    #[test]
    fn a_file_ffmpeg_cannot_read_either_reports_the_original_error() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/not-audio.txt");
        let error = Decoder::open(&path).err().expect("text is not audio");
        assert!(
            error.to_string().contains("no suitable format reader"),
            "{error}"
        );
    }
}

#[cfg(not(feature = "ffmpeg"))]
#[test]
fn without_ffmpeg_the_extra_formats_are_unsupported_not_a_crash() {
    for file in ["sine.opus", "sine.wv", "sine.ape", "sine-dsd64.dsf"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(file);
        assert!(Decoder::open(&path).is_err(), "{file}");
    }
}
