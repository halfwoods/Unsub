//! Loudness measured through the public API: known levels, gating, albums, and every format.

use std::f64::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use unsub_playback::{ChannelLayout, Decoder, Error, Loudness, album_loudness, analyze_loudness};

const RATE: u32 = 48_000;
/// Left, right, centre, LFE, rear pair, and for 7.1 the side pair.
const MASK_5_1: u32 = 0x3F;
const MASK_7_1: u32 = 0x63F;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A scratch directory removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("unsub-loudness-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `seconds` of a 997 Hz sine whose peak is `dbfs`; the tone EBU Tech 3341 calibrates with.
fn tone(dbfs: f64, seconds: usize) -> Vec<f64> {
    let amplitude = 10f64.powf(dbfs / 20.0);
    (0..RATE as usize * seconds)
        .map(|n| amplitude * (TAU * 997.0 * n as f64 / f64::from(RATE)).sin())
        .collect()
}

fn silence(seconds: usize) -> Vec<f64> {
    vec![0.0; RATE as usize * seconds]
}

/// Writes 16-bit PCM, one entry of `channels` per channel; `mask` is the speaker mask for more
/// than two channels.
fn write_wav(path: &Path, channels: &[&[f64]], mask: u32) {
    let count = channels.len() as u16;
    let frames = channels[0].len();
    let extensible = mask != 0;
    let fmt_len = if extensible { 40 } else { 16 };
    let data_len = (frames * usize::from(count) * 2) as u32;
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((4 + 8 + fmt_len + 8 + data_len).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(fmt_len.to_le_bytes());
    wav.extend(if extensible { 0xFFFEu16 } else { 1 }.to_le_bytes());
    wav.extend(count.to_le_bytes());
    wav.extend(RATE.to_le_bytes());
    wav.extend((RATE * u32::from(count) * 2).to_le_bytes());
    wav.extend((count * 2).to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    if extensible {
        wav.extend(22u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(mask.to_le_bytes());
        wav.extend([
            1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
        ]);
    }
    wav.extend(b"data");
    wav.extend(data_len.to_le_bytes());
    for frame in 0..frames {
        for channel in channels {
            let sample = (channel[frame] * 32768.0).round().clamp(-32768.0, 32767.0);
            wav.extend((sample as i16).to_le_bytes());
        }
    }
    fs::write(path, wav).unwrap();
}

fn stereo(path: &Path, samples: &[f64]) {
    write_wav(path, &[samples, samples], 0);
}

fn measure(path: &Path) -> Loudness {
    analyze_loudness(path, &mut |_, _| {}, &AtomicBool::new(false))
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn assert_near(actual: f64, expected: f64, tolerance: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} should be within {tolerance} of {expected}"
    );
}

#[test]
fn a_calibration_tone_measures_its_level() {
    let scratch = Scratch::new("calibration");
    let path = scratch.path("tone.wav");
    stereo(&path, &tone(-23.0, 10));
    let loudness = measure(&path);
    assert_near(loudness.integrated_lufs, -23.0, 0.05, "loudness");
    assert_near(loudness.true_peak_dbtp, -23.0, 0.1, "true peak");
    assert_eq!(loudness.duration_ms, 10_000);
}

#[test]
fn mono_counts_as_the_same_audio_on_both_speakers() {
    let scratch = Scratch::new("mono");
    let path = scratch.path("mono.wav");
    write_wav(&path, &[&tone(-23.0, 5)], 0);
    assert_near(measure(&path).integrated_lufs, -23.0, 0.05, "mono");
}

#[test]
fn true_peak_sees_between_the_samples() {
    // A quarter-rate sine at 45 degrees lands every sample on 0.707 of its crest.
    let samples: Vec<f64> = (0..RATE as usize)
        .map(|n| 0.9 * (TAU * 12_000.0 * n as f64 / f64::from(RATE) + TAU / 8.0).sin())
        .collect();
    let scratch = Scratch::new("true-peak");
    let path = scratch.path("peak.wav");
    stereo(&path, &samples);
    let sample_peak = 20.0 * (0.9f64 * 0.5f64.sqrt()).log10();
    let loudness = measure(&path);
    assert_near(
        loudness.true_peak_dbtp,
        20.0 * 0.9f64.log10(),
        0.2,
        "true peak",
    );
    assert!(loudness.true_peak_dbtp > sample_peak + 2.0, "{loudness:?}");
}

#[test]
fn passages_far_below_the_programme_are_gated_out() {
    let scratch = Scratch::new("gating");
    let path = scratch.path("gated.wav");
    let samples = [tone(-23.0, 10), tone(-53.0, 10)].concat();
    stereo(&path, &samples);
    // Averaging both halves would give -26. The blocks that straddle the change are loud enough
    // to count and pull the result down a little.
    assert_near(measure(&path).integrated_lufs, -23.0, 0.1, "loudness");
}

#[test]
fn silence_has_no_loudness_and_still_serializes() {
    let scratch = Scratch::new("silence");
    let path = scratch.path("silence.wav");
    stereo(&path, &silence(2));
    let loudness = measure(&path);
    assert_eq!(loudness.integrated_lufs, f64::NEG_INFINITY);
    assert_eq!(loudness.true_peak_dbtp, f64::NEG_INFINITY);
    let json = serde_json::to_string(&loudness).unwrap();
    assert_eq!(serde_json::from_str::<Loudness>(&json).unwrap(), loudness);
}

/// An album measured from its tracks' stored data must equal the album measured as one file.
fn assert_album_matches_concatenation(name: &str, levels_db: &[f64], expected: f64) {
    let scratch = Scratch::new(name);
    let mut tracks = Vec::new();
    let mut whole = Vec::new();
    for (n, &level) in levels_db.iter().enumerate() {
        let samples = tone(level, 20);
        let path = scratch.path(&format!("{n}.wav"));
        stereo(&path, &samples);
        // Store and reload as a library would, so the stored form is what is combined.
        let stored = serde_json::to_string(&measure(&path)).unwrap();
        tracks.push(serde_json::from_str::<Loudness>(&stored).unwrap());
        whole.extend(samples);
    }
    let path = scratch.path("album.wav");
    stereo(&path, &whole);
    let album = album_loudness(&tracks);
    assert_near(
        album.integrated_lufs,
        measure(&path).integrated_lufs,
        0.03,
        name,
    );
    assert_near(album.integrated_lufs, expected, 0.05, name);
    assert_eq!(album.duration_ms, 20_000 * levels_db.len() as u64);
}

#[test]
fn an_album_is_the_energy_mean_of_its_tracks() {
    // Two tones 10 LU apart: the quiet one is within the relative gate, so it counts. Averaging
    // the two tracks' figures would say -28 LUFS; the programme is -25.6.
    assert_album_matches_concatenation("close-tracks", &[-23.0, -33.0], -25.6);
}

#[test]
fn a_quiet_track_is_gated_out_of_the_album_it_would_otherwise_drag_down() {
    // 22 LU apart, the quiet one falls below the album's relative gate. Weighting the tracks'
    // own figures by duration would say -26; the programme is -23.
    assert_album_matches_concatenation("far-tracks", &[-23.0, -45.0], -23.0);
}

#[test]
fn an_album_keeps_the_highest_peak_and_the_total_length() {
    let scratch = Scratch::new("album-peak");
    let tracks: Vec<Loudness> = [-23.0, -14.0, -30.0]
        .iter()
        .enumerate()
        .map(|(n, &level)| {
            let path = scratch.path(&format!("{n}.wav"));
            stereo(&path, &tone(level, 3));
            measure(&path)
        })
        .collect();
    let album = album_loudness(&tracks);
    assert_near(album.true_peak_dbtp, -14.0, 0.1, "peak");
    assert_eq!(album.duration_ms, 9_000);
}

#[test]
fn surround_channels_are_weighted_and_the_lfe_is_left_out() {
    let scratch = Scratch::new("surround");
    // The loudness of the same tone with it in just one channel of a six-channel file.
    let in_channel = |index: usize| {
        let path = scratch.path(&format!("5.1-{index}.wav"));
        let quiet = silence(10);
        let loud = tone(-23.0, 10);
        let channels: Vec<&[f64]> = (0..6)
            .map(|n| if n == index { &loud[..] } else { &quiet[..] })
            .collect();
        write_wav(&path, &channels, MASK_5_1);
        assert_eq!(
            Decoder::open(&path).unwrap().channel_layout(),
            ChannelLayout::Surround5_1
        );
        measure(&path)
    };
    let front = in_channel(0);
    // A single front channel is half the power of the stereo calibration tone.
    assert_near(front.integrated_lufs, -26.0, 0.05, "front left");
    assert_near(
        in_channel(1).integrated_lufs,
        front.integrated_lufs,
        0.01,
        "front right",
    );
    assert_near(
        in_channel(2).integrated_lufs,
        front.integrated_lufs,
        0.01,
        "centre",
    );
    // BS.1770-4 gives the surround channels +1.5 dB.
    assert_near(
        in_channel(4).integrated_lufs,
        front.integrated_lufs + 1.5,
        0.05,
        "left surround",
    );
    assert_near(
        in_channel(5).integrated_lufs,
        front.integrated_lufs + 1.5,
        0.05,
        "right surround",
    );
    // The LFE is not part of the loudness, but it can still clip.
    let lfe = in_channel(3);
    assert_eq!(lfe.integrated_lufs, f64::NEG_INFINITY);
    assert_near(lfe.true_peak_dbtp, -23.0, 0.1, "lfe peak");
}

#[test]
fn seven_one_boosts_the_side_pair_but_not_the_rear_pair() {
    let scratch = Scratch::new("seven-one");
    let in_channel = |index: usize| {
        let path = scratch.path(&format!("7.1-{index}.wav"));
        let quiet = silence(10);
        let loud = tone(-23.0, 10);
        let channels: Vec<&[f64]> = (0..8)
            .map(|n| if n == index { &loud[..] } else { &quiet[..] })
            .collect();
        write_wav(&path, &channels, MASK_7_1);
        assert_eq!(
            Decoder::open(&path).unwrap().channel_layout(),
            ChannelLayout::Surround7_1
        );
        measure(&path).integrated_lufs
    };
    let front = in_channel(0);
    assert_near(in_channel(4), front, 0.05, "rear left");
    assert_near(in_channel(5), front, 0.05, "rear right");
    assert_near(in_channel(6), front + 1.5, 0.05, "side left");
    assert_near(in_channel(7), front + 1.5, 0.05, "side right");
}

/// The loudest sample the decoder hands to playback, in dBFS.
fn decoded_peak_db(path: &Path) -> f64 {
    let mut decoder = Decoder::open(path).unwrap();
    let mut peak = 0f32;
    while let Some(chunk) = decoder.next_chunk().unwrap() {
        peak = chunk
            .iter()
            .fold(peak, |peak, sample| peak.max(sample.abs()));
    }
    20.0 * f64::from(peak).log10()
}

#[test]
fn the_true_peak_is_that_of_the_audio_playback_would_hear_in_every_format() {
    let files = [
        "sine.flac",
        "sine-24bit.flac",
        "sine-24bit-96k.flac",
        "sine-alac.m4a",
        "sine-22k.aiff",
        "sine-mono.wav",
        "sine-aac.m4a",
        "sine.mp3",
        "sine.ogg",
        #[cfg(feature = "ffmpeg")]
        "sine.wv",
        #[cfg(feature = "ffmpeg")]
        "sine.ape",
        #[cfg(feature = "ffmpeg")]
        "sine.opus",
        #[cfg(feature = "ffmpeg")]
        "sine-dsd64.dsf",
        #[cfg(feature = "ffmpeg")]
        "sine-dsd64.dff",
    ];
    for file in files {
        let path = fixture(file);
        let heard = decoded_peak_db(&path);
        let measured = measure(&path).true_peak_dbtp;
        // Never below the highest sample, and for a low sine not far above it.
        assert!(
            measured >= heard - 0.01 && measured <= heard + 0.5,
            "{file}: measured {measured} dBTP, decoded peak {heard} dBFS"
        );
    }
}

#[test]
fn lossless_copies_of_one_sine_measure_alike_and_lossy_ones_nearly() {
    let reference = measure(&fixture("sine.flac"));
    assert_near(reference.true_peak_dbtp, -6.02, 0.1, "reference peak");
    assert!(reference.integrated_lufs.is_finite());
    let files = [
        ("sine-alac.m4a", 0.05),
        ("sine-22k.aiff", 0.05),
        ("sine-mono.wav", 0.05),
        ("sine-aac.m4a", 0.5),
        ("sine.mp3", 0.5),
        ("sine.ogg", 0.5),
        #[cfg(feature = "ffmpeg")]
        ("sine.wv", 0.05),
        #[cfg(feature = "ffmpeg")]
        ("sine.ape", 0.05),
        #[cfg(feature = "ffmpeg")]
        ("sine.opus", 0.5),
    ];
    for (file, tolerance) in files {
        let loudness = measure(&fixture(file));
        assert_near(
            loudness.integrated_lufs,
            reference.integrated_lufs,
            tolerance,
            file,
        );
    }
}

#[cfg(feature = "ffmpeg")]
#[test]
fn dsd_is_measured_after_conversion_to_pcm() {
    // 50 ms is shorter than one 400 ms block, so there is no loudness to report; the length
    // is that of the converted audio.
    for file in ["sine-dsd64.dsf", "sine-dsd64.dff"] {
        let loudness = measure(&fixture(file));
        assert_eq!(loudness.integrated_lufs, f64::NEG_INFINITY, "{file}");
        assert!(loudness.true_peak_dbtp > -7.0, "{file}: {loudness:?}");
        assert_eq!(loudness.duration_ms, 50, "{file}");
    }
}

#[test]
fn cancelling_stops_the_measurement() {
    let result = analyze_loudness(
        &fixture("sine.flac"),
        &mut |_, _| {},
        &AtomicBool::new(true),
    );
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
}

#[test]
fn progress_arrives_about_once_a_second_and_ends_complete() {
    let scratch = Scratch::new("progress");
    let path = scratch.path("ten-seconds.wav");
    stereo(&path, &tone(-23.0, 10));
    let mut reports = Vec::new();
    analyze_loudness(
        &path,
        &mut |done, total| reports.push((done, total)),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!((10..=12).contains(&reports.len()), "{reports:?}");
    assert!(
        reports.iter().all(|&(_, total)| total == 10_000),
        "{reports:?}"
    );
    assert!(
        reports.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "{reports:?}"
    );
    assert_eq!(reports.last(), Some(&(10_000, 10_000)));
}
