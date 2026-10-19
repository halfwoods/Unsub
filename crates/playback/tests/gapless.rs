//! Gapless playback needs the decoder to hand back exactly the audio the encoder was given:
//! no priming silence in front and no padding behind.

use std::path::PathBuf;

use antistreaming_playback::Decoder;

/// Frames of 440 Hz sine in every gapless fixture; not a multiple of any codec frame size.
const FRAMES: usize = 43_210;
const SAMPLE_RATE: f32 = 44_100.0;

/// Each is a different source of trim information: the LAME header in the mp3, an ffmpeg
/// edit list, and the `iTunSMPB` tag Apple's encoder writes.
const FIXTURES: &[&str] = &["gapless.mp3", "gapless-ffmpeg.m4a", "gapless-itunes.m4a"];

fn open(file: &str) -> Decoder {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(file);
    Decoder::open(&path).unwrap_or_else(|error| panic!("{file} should open: {error}"))
}

fn decode_to_end(decoder: &mut Decoder) -> Vec<f32> {
    let mut samples = Vec::new();
    while let Some(chunk) = decoder.next_chunk().expect("decoding should not fail") {
        samples.extend_from_slice(chunk);
    }
    samples
}

fn left_channel(samples: &[f32]) -> impl Iterator<Item = f32> + '_ {
    samples.iter().step_by(2).copied()
}

fn peak(samples: impl Iterator<Item = f32>) -> f32 {
    samples.fold(0.0, |peak, sample| peak.max(sample.abs()))
}

#[test]
fn decoded_length_is_the_true_sample_count() {
    for file in FIXTURES {
        let frames = decode_to_end(&mut open(file)).len() / 2;
        assert_eq!(frames, FRAMES, "{file}");
    }
}

#[test]
fn reported_duration_matches_the_trimmed_length() {
    for file in FIXTURES {
        let duration_ms = open(file).duration_ms();
        assert!(
            matches!(duration_ms, Some(979 | 980)),
            "{file}: {duration_ms:?}"
        );
    }
}

#[test]
fn the_audio_starts_and_ends_without_encoder_silence() {
    for file in FIXTURES {
        let samples = decode_to_end(&mut open(file));
        let edge = 600 * 2;
        let head = peak(left_channel(&samples[..edge]));
        let tail = peak(left_channel(&samples[samples.len() - edge..]));
        assert!(head > 0.3, "{file}: head peak {head} (priming left in?)");
        assert!(tail > 0.3, "{file}: tail peak {tail} (padding left in?)");
    }
}

#[test]
fn the_waveform_is_not_shifted_by_the_encoder_delay() {
    for file in FIXTURES {
        let samples = decode_to_end(&mut open(file));
        // Lossy coding smears the very ends, so compare the middle against the ideal sine. A
        // shift of even a few frames is far above the coding noise (about 0.04 RMS at worst).
        let middle = 300..FRAMES - 300;
        let squared_error: f32 = left_channel(&samples)
            .enumerate()
            .filter(|(n, _)| middle.contains(n))
            .map(|(n, sample)| {
                let phase = 2.0 * std::f32::consts::PI * 440.0 * n as f32 / SAMPLE_RATE;
                (sample - 0.5 * phase.sin()).powi(2)
            })
            .sum();
        let rms = (squared_error / middle.len() as f32).sqrt();
        assert!(rms < 0.06, "{file}: RMS error {rms}");
    }
}

#[test]
fn seeking_counts_from_the_first_real_frame() {
    for file in FIXTURES {
        let mut decoder = open(file);
        assert_eq!(decoder.seek(500).unwrap(), 500, "{file}");
        let remaining = decode_to_end(&mut decoder).len() / 2;
        assert_eq!(remaining, FRAMES - 22_050, "{file}");

        assert_eq!(decoder.seek(0).unwrap(), 0, "{file}");
        let whole = decode_to_end(&mut decoder).len() / 2;
        assert_eq!(
            whole, FRAMES,
            "{file}: seeking to the start must not trim twice"
        );
    }
}

#[test]
fn seeking_past_the_trimmed_end_ends_the_stream() {
    for file in FIXTURES {
        let mut decoder = open(file);
        // 990 ms is beyond the 979 ms of audio but inside the encoder's padding.
        if decoder.seek(990).is_ok() {
            assert_eq!(decode_to_end(&mut decoder).len(), 0, "{file}");
        }
    }
}
