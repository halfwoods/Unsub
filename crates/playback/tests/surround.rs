//! Surround files must come out of the decoder labelled and in the channel order the downmix
//! assumes: front left and right, centre, LFE, then the rear pair and the side pair.

use std::path::PathBuf;

use unsub_playback::{ChannelLayout, Decoder};

/// Each channel of the fixtures is a different constant, so a swapped pair shows up directly.
const FIVE_ONE: [f32; 6] = [0.05, 0.10, 0.15, 0.20, 0.25, 0.30];
const SEVEN_ONE: [f32; 8] = [0.05, 0.10, 0.15, 0.20, 0.25, 0.30, 0.35, 0.40];

fn first_frame(file: &str) -> (ChannelLayout, Vec<f32>) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(file);
    let mut decoder = Decoder::open(&path).unwrap_or_else(|error| panic!("{file}: {error}"));
    let channels = usize::from(decoder.channels());
    let chunk = decoder.next_chunk().unwrap().expect("some audio");
    // Skip the first few frames: the encoder's first block can carry a start-up transient.
    let frame = chunk[100 * channels..101 * channels].to_vec();
    (decoder.channel_layout(), frame)
}

fn assert_close(actual: &[f32], expected: &[f32], file: &str) {
    for (channel, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (a - e).abs() < 1e-3,
            "{file}: channel {channel} is {a}, expected {e}"
        );
    }
}

#[test]
fn five_one_is_labelled_and_ordered() {
    let (layout, frame) = first_frame("surround-5.1.flac");
    assert_eq!(layout, ChannelLayout::Surround5_1);
    assert_close(&frame, &FIVE_ONE, "surround-5.1.flac");
}

#[test]
fn seven_one_is_labelled_and_ordered() {
    let (layout, frame) = first_frame("surround-7.1.flac");
    assert_eq!(layout, ChannelLayout::Surround7_1);
    assert_close(&frame, &SEVEN_ONE, "surround-7.1.flac");
}

#[test]
fn stereo_and_mono_files_report_their_layout() {
    assert_eq!(first_frame("sine.flac").0, ChannelLayout::Stereo);
    assert_eq!(first_frame("sine-mono.wav").0, ChannelLayout::Mono);
}
