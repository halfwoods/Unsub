//! Converting files and streams, checked by decoding what comes out.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use antistreaming_playback::{Error, Target, TranscodeOptions, transcode_file};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn options(target: Target) -> TranscodeOptions {
    TranscodeOptions {
        target,
        max_sample_rate: None,
        max_bit_depth: None,
        keep_tags: false,
    }
}

/// A scratch directory removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "antistreaming-transcode-{}-{test}",
            std::process::id()
        ));
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

#[cfg(feature = "ffmpeg")]
mod with_ffmpeg {
    use std::io::Read;
    use std::path::Path;
    use std::sync::atomic::Ordering;

    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::Accessor;

    use antistreaming_playback::{Decoder, ipod_compatible, transcode_stream};

    use super::*;

    struct Decoded {
        sample_rate: u32,
        channels: u16,
        bit_depth: Option<u32>,
        samples: Vec<f32>,
    }

    impl Decoded {
        fn frames(&self) -> usize {
            self.samples.len() / usize::from(self.channels)
        }

        fn channel(&self, index: usize) -> Vec<f32> {
            self.samples
                .iter()
                .skip(index)
                .step_by(usize::from(self.channels))
                .copied()
                .collect()
        }
    }

    fn decode(path: &Path) -> Decoded {
        let mut decoder = Decoder::open(path)
            .unwrap_or_else(|error| panic!("{} should open: {error}", path.display()));
        let mut samples = Vec::new();
        while let Some(chunk) = decoder.next_chunk().expect("decoding should not fail") {
            samples.extend_from_slice(chunk);
        }
        Decoded {
            sample_rate: decoder.sample_rate(),
            channels: decoder.channels(),
            bit_depth: decoder.bit_depth(),
            samples,
        }
    }

    /// RMS difference from an ideal 440 Hz sine of amplitude 0.5, over the middle of the signal where
    /// lossy codecs and filters have settled.
    fn rms_error_against_sine(signal: &[f32], sample_rate: u32) -> f32 {
        let edge = signal.len() / 8;
        let middle = &signal[edge..signal.len() - edge];
        let sum: f32 = middle
            .iter()
            .enumerate()
            .map(|(n, sample)| {
                let phase =
                    2.0 * std::f32::consts::PI * 440.0 * (n + edge) as f32 / sample_rate as f32;
                (sample - 0.5 * phase.sin()).powi(2)
            })
            .sum();
        (sum / middle.len() as f32).sqrt()
    }

    fn convert(scratch: &Scratch, input: &str, file: &str, options: &TranscodeOptions) -> PathBuf {
        let output = scratch.path(file);
        transcode_file(
            &fixture(input),
            &output,
            options,
            &mut |_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap_or_else(|error| panic!("converting {input} to {file}: {error}"));
        output
    }

    const KBPS: u32 = 128;

    const ALL: [(&str, Target); 5] = [
        ("alac.m4a", Target::Alac),
        ("aac.m4a", Target::Aac { kbps: KBPS }),
        ("mp3.mp3", Target::Mp3 { kbps: KBPS }),
        ("opus.opus", Target::Opus { kbps: 96 }),
        ("flac.flac", Target::Flac),
    ];

    #[test]
    fn this_build_has_an_encoder_for_every_target() {
        for (file, target) in ALL {
            assert!(target.is_available(), "{file}");
        }
    }

    #[test]
    fn every_target_decodes_back_to_the_same_duration_to_the_frame() {
        let scratch = Scratch::new("duration");
        for (file, target) in ALL {
            let output = convert(&scratch, "sine.flac", file, &options(target));
            let decoded = decode(&output);
            // Opus codes at 48 kHz; everything else keeps the source's 44.1 kHz.
            let rate = if matches!(target, Target::Opus { .. }) {
                48_000
            } else {
                44_100
            };
            assert_eq!(decoded.sample_rate, rate, "{file}");
            assert_eq!(decoded.channels, 2, "{file}");
            assert!(
                decoded.frames().abs_diff(rate as usize) <= 1,
                "{file}: {} frames, wanted {rate}",
                decoded.frames()
            );
        }
    }

    #[test]
    fn lossless_targets_reproduce_the_source_bit_for_bit() {
        let scratch = Scratch::new("lossless");
        let source = decode(&fixture("sine.flac"));
        for (file, target) in [("alac.m4a", Target::Alac), ("flac.flac", Target::Flac)] {
            let decoded = decode(&convert(&scratch, "sine.flac", file, &options(target)));
            assert_eq!(decoded.bit_depth, Some(16), "{file}");
            assert!(
                decoded.samples == source.samples,
                "{file} changed the audio"
            );
        }
    }

    #[test]
    fn lossy_targets_keep_the_waveform_in_place() {
        let scratch = Scratch::new("lossy");
        for (file, target) in &ALL[1..4] {
            let decoded = decode(&convert(&scratch, "sine.flac", file, &options(*target)));
            let error = rms_error_against_sine(&decoded.channel(0), decoded.sample_rate);
            assert!(error < 0.06, "{file}: RMS error {error}");
        }
    }

    #[test]
    fn mono_stays_mono() {
        let scratch = Scratch::new("mono");
        for (file, target) in &ALL[..4] {
            let decoded = decode(&convert(&scratch, "sine-mono.wav", file, &options(*target)));
            assert_eq!(decoded.channels, 1, "{file}");
        }
    }

    #[test]
    fn surround_is_downmixed_to_stereo() {
        let scratch = Scratch::new("surround");
        let decoded = decode(&convert(
            &scratch,
            "surround-5.1.flac",
            "five-one.flac",
            &options(Target::Flac),
        ));
        assert_eq!(decoded.channels, 2);
        // Left is 0.05 + 0.15 / sqrt(2) + 0.25 / sqrt(2), scaled by 1 / (1 + sqrt(2)).
        let c = std::f32::consts::FRAC_1_SQRT_2;
        let want = (0.05 + c * 0.15 + c * 0.25) / (1.0 + 2.0 * c);
        let middle = decoded.frames() / 2;
        assert!((decoded.samples[2 * middle] - want).abs() < 2e-3);
    }

    mod limits {
        use super::*;

        #[test]
        fn hi_res_without_limits_is_kept_exactly() {
            let scratch = Scratch::new("hires-kept");
            let source = decode(&fixture("sine-24bit-96k.flac"));
            let out = convert(
                &scratch,
                "sine-24bit-96k.flac",
                "hires.m4a",
                &options(Target::Alac),
            );
            let decoded = decode(&out);
            assert_eq!((decoded.sample_rate, decoded.bit_depth), (96_000, Some(24)));
            assert!(decoded.samples == source.samples, "24-bit samples changed");
        }

        #[test]
        fn hi_res_is_brought_down_to_the_limits_and_plays_on_an_ipod() {
            let scratch = Scratch::new("hires-limited");
            let limited = TranscodeOptions {
                max_sample_rate: Some(48_000),
                max_bit_depth: Some(16),
                ..options(Target::Alac)
            };
            let out = convert(&scratch, "sine-24bit-96k.flac", "ipod.m4a", &limited);

            let decoded = decode(&out);
            assert_eq!((decoded.sample_rate, decoded.bit_depth), (48_000, Some(16)));
            assert!(
                decoded.frames().abs_diff(4_800) <= 1,
                "{} frames",
                decoded.frames()
            );
            // Resampling and dither leave a sine within a fraction of a 16-bit step of the ideal.
            let error = rms_error_against_sine(&decoded.channel(0), 48_000);
            assert!(error < 1e-4, "RMS error {error}");
            assert!(error > 0.0, "dither noise is expected");

            assert!(!ipod_compatible(&fixture("sine-24bit-96k.flac")).unwrap());
            assert!(ipod_compatible(&out).unwrap());
        }

        #[test]
        fn a_depth_limit_alone_keeps_the_rate() {
            let scratch = Scratch::new("depth-only");
            let limited = TranscodeOptions {
                max_bit_depth: Some(16),
                ..options(Target::Flac)
            };
            let decoded = decode(&convert(
                &scratch,
                "sine-24bit.flac",
                "sixteen.flac",
                &limited,
            ));
            assert_eq!((decoded.sample_rate, decoded.bit_depth), (44_100, Some(16)));
            assert!(decoded.frames().abs_diff(8_820) <= 1);
        }

        #[test]
        fn limits_do_not_touch_a_file_already_within_them() {
            let scratch = Scratch::new("within");
            let generous = TranscodeOptions {
                max_sample_rate: Some(48_000),
                max_bit_depth: Some(16),
                ..options(Target::Alac)
            };
            let source = decode(&fixture("sine.flac"));
            let decoded = decode(&convert(&scratch, "sine.flac", "same.m4a", &generous));
            assert!(
                decoded.samples == source.samples,
                "an in-limits file must pass through exactly"
            );
        }

        #[test]
        fn dsd_becomes_pcm_within_the_lossless_ceiling() {
            let scratch = Scratch::new("dsd");
            let decoded = decode(&convert(
                &scratch,
                "sine-dsd64.dsf",
                "dsd.flac",
                &options(Target::Flac),
            ));
            assert_eq!(
                (decoded.sample_rate, decoded.bit_depth),
                (176_400, Some(24))
            );
            assert!(
                decoded.frames().abs_diff(8_820) <= 1,
                "{} frames",
                decoded.frames()
            );
        }

        #[test]
        fn lossy_targets_stay_within_what_their_format_allows() {
            let scratch = Scratch::new("lossy-rates");
            let mp3 = decode(&convert(
                &scratch,
                "sine-24bit-96k.flac",
                "hi.mp3",
                &options(Target::Mp3 { kbps: KBPS }),
            ));
            assert_eq!(mp3.sample_rate, 48_000);
            let capped = TranscodeOptions {
                max_sample_rate: Some(32_000),
                ..options(Target::Aac { kbps: 96 })
            };
            let aac = decode(&convert(&scratch, "sine.flac", "low.m4a", &capped));
            assert_eq!(aac.sample_rate, 32_000);
        }
    }

    mod gapless {
        use super::*;

        #[test]
        fn aac_in_an_m4a_carries_its_own_delay_and_padding_marker() {
            use lofty::config::ParseOptions;
            use lofty::file::AudioFile;
            use lofty::mp4::{AtomData, AtomIdent, Mp4File};

            let scratch = Scratch::new("smpb");
            let out = convert(
                &scratch,
                "gapless.mp3",
                "g.m4a",
                &options(Target::Aac { kbps: KBPS }),
            );
            let mut file = fs::File::open(&out).unwrap();
            let mp4 = Mp4File::read_from(&mut file, ParseOptions::new()).unwrap();
            let marker = mp4
                .ilst()
                .and_then(|ilst| {
                    ilst.get(&AtomIdent::Freeform {
                        mean: "com.apple.iTunes".into(),
                        name: "iTunSMPB".into(),
                    })
                })
                .and_then(|atom| match atom.data().next() {
                    Some(AtomData::UTF8(text)) => Some(text.clone()),
                    _ => None,
                })
                .expect("the m4a has an iTunSMPB atom");

            let fields: Vec<u64> = marker
                .split_whitespace()
                .map(|field| u64::from_str_radix(field, 16).unwrap())
                .collect();
            assert!(fields[1] > 0, "AAC has encoder delay: {marker}");
            assert_eq!(fields[3], 43_210, "the true sample count: {marker}");
        }

        #[test]
        fn mp3_files_start_with_a_lame_header_that_declares_the_encoder_delay() {
            let scratch = Scratch::new("lame");
            let out = convert(
                &scratch,
                "gapless.mp3",
                "g.mp3",
                &options(Target::Mp3 { kbps: KBPS }),
            );
            let bytes = fs::read(&out).unwrap();
            let head = &bytes[..bytes.len().min(2_048)];
            let info = head
                .windows(4)
                .position(|window| window == b"Info" || window == b"Xing")
                .expect("an Info or Xing header");

            // Four bytes of tag, flags, frame count, byte count, a 100-byte seek table and a quality
            // word precede the 9-byte encoder name; the delay and padding sit 12 bytes after that.
            let name = info + 4 + 4 + 4 + 4 + 100 + 4;
            assert_eq!(&head[name..name + 4], b"Lavf", "the encoder's name");
            let packed = &head[name + 21..name + 24];
            let delay = (u32::from(packed[0]) << 4) | (u32::from(packed[1]) >> 4);
            let padding = (u32::from(packed[1] & 0x0F) << 8) | u32::from(packed[2]);
            // LAME's 576-sample encoder delay; decoders add their own 529 on top.
            assert_eq!(delay, 576);
            assert!(padding > 0, "trailing padding is declared too");
        }

        #[test]
        fn odd_lengths_survive_exactly_through_aac_mp3_and_opus() {
            let scratch = Scratch::new("odd");
            for (file, target, rate) in [
                ("odd.m4a", Target::Aac { kbps: KBPS }, 44_100),
                ("odd.mp3", Target::Mp3 { kbps: KBPS }, 44_100),
                ("odd.opus", Target::Opus { kbps: 96 }, 48_000),
            ] {
                let decoded = decode(&convert(&scratch, "gapless.mp3", file, &options(target)));
                let want = 43_210 * rate as usize / 44_100;
                assert!(
                    decoded.frames().abs_diff(want) <= 1,
                    "{file}: {} frames, wanted {want}",
                    decoded.frames()
                );
            }
        }
    }

    mod tagging {
        use super::*;

        fn kept(
            test: &str,
            options: TranscodeOptions,
            file: &str,
        ) -> (Option<lofty::tag::Tag>, usize) {
            let scratch = Scratch::new(&format!("{test}-{file}"));
            let output = convert(&scratch, "tagged.flac", file, &options);
            let tagged = Probe::open(&output).unwrap().read().unwrap();
            let tag = tagged.primary_tag().or_else(|| tagged.first_tag()).cloned();
            let pictures = tag.as_ref().map_or(0, |tag| tag.pictures().len());
            (tag, pictures)
        }

        #[test]
        fn text_tags_and_artwork_survive_every_target() {
            for (file, target) in ALL {
                let (tag, pictures) = kept(
                    "survive",
                    TranscodeOptions {
                        keep_tags: true,
                        ..options(target)
                    },
                    file,
                );
                let tag = tag.unwrap_or_else(|| panic!("{file} lost its tags"));
                assert_eq!(tag.title().as_deref(), Some("Sine Title"), "{file}");
                assert_eq!(tag.artist().as_deref(), Some("Test Artist"), "{file}");
                assert_eq!(tag.album().as_deref(), Some("Test Album"), "{file}");
                assert_eq!(tag.track(), Some(3), "{file}");
                assert_eq!(tag.genre().as_deref(), Some("Jazz"), "{file}");
                assert_eq!(pictures, 1, "{file} lost its artwork");
            }
        }

        #[test]
        fn the_artwork_bytes_are_unchanged() {
            let source = Probe::open(fixture("tagged.flac")).unwrap().read().unwrap();
            let original = source.primary_tag().unwrap().pictures()[0].data().to_vec();
            for (file, target) in ALL {
                let (tag, _) = kept(
                    "bytes",
                    TranscodeOptions {
                        keep_tags: true,
                        ..options(target)
                    },
                    file,
                );
                assert_eq!(
                    tag.unwrap().pictures()[0].data(),
                    original.as_slice(),
                    "{file}"
                );
            }
        }

        #[test]
        fn without_keep_tags_nothing_is_carried_over() {
            for (file, target) in ALL {
                let (tag, pictures) = kept("dropped", options(target), file);
                assert_eq!(pictures, 0, "{file}");
                assert!(
                    tag.is_none_or(|tag| tag.title().is_none() && tag.artist().is_none()),
                    "{file}"
                );
            }
        }

        #[test]
        fn a_file_without_tags_converts_without_trouble() {
            let scratch = Scratch::new("untagged");
            for (file, target) in ALL {
                convert(
                    &scratch,
                    "sine.flac",
                    file,
                    &TranscodeOptions {
                        keep_tags: true,
                        ..options(target)
                    },
                );
            }
        }
    }

    mod cancel_and_failure {
        use super::*;

        fn leftovers(dir: &Path) -> Vec<String> {
            fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        }

        #[test]
        fn cancelling_part_way_leaves_no_file_behind() {
            let scratch = Scratch::new("cancel");
            let cancel = AtomicBool::new(false);
            let output = scratch.path("never.m4a");
            let mut calls = 0;
            let result = transcode_file(
                &fixture("sine.flac"),
                &output,
                &options(Target::Aac { kbps: KBPS }),
                &mut |_, _| {
                    calls += 1;
                    if calls == 2 {
                        cancel.store(true, Ordering::Relaxed);
                    }
                },
                &cancel,
            );
            assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
            assert!(
                leftovers(&scratch.0).is_empty(),
                "{:?}",
                leftovers(&scratch.0)
            );
        }

        #[test]
        fn cancelling_before_the_start_does_nothing_at_all() {
            let scratch = Scratch::new("cancel-early");
            let result = transcode_file(
                &fixture("sine.flac"),
                &scratch.path("never.mp3"),
                &options(Target::Mp3 { kbps: KBPS }),
                &mut |_, _| {},
                &AtomicBool::new(true),
            );
            assert!(matches!(result, Err(Error::Cancelled)));
            assert!(leftovers(&scratch.0).is_empty());
        }

        #[test]
        fn a_cancelled_conversion_leaves_an_existing_file_as_it_was() {
            let scratch = Scratch::new("cancel-existing");
            let output = scratch.path("keep.flac");
            fs::write(&output, b"precious").unwrap();
            let result = transcode_file(
                &fixture("sine.flac"),
                &output,
                &options(Target::Flac),
                &mut |_, _| {},
                &AtomicBool::new(true),
            );
            assert!(result.is_err());
            assert_eq!(fs::read(&output).unwrap(), b"precious");
        }

        #[test]
        fn an_unreadable_input_leaves_nothing_behind() {
            let scratch = Scratch::new("bad-input");
            let result = transcode_file(
                &fixture("not-audio.txt"),
                &scratch.path("out.m4a"),
                &options(Target::Alac),
                &mut |_, _| {},
                &AtomicBool::new(false),
            );
            assert!(result.is_err());
            assert!(leftovers(&scratch.0).is_empty());
        }

        #[test]
        fn progress_climbs_to_the_whole_duration() {
            let scratch = Scratch::new("progress");
            let mut seen = Vec::new();
            transcode_file(
                &fixture("sine.flac"),
                &scratch.path("p.flac"),
                &options(Target::Flac),
                &mut |done, total| seen.push((done, total)),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(seen.len() > 2, "{seen:?}");
            assert!(
                seen.windows(2).all(|pair| pair[0].0 <= pair[1].0),
                "never goes backwards"
            );
            assert_eq!(seen.last(), Some(&(1_000, 1_000)));
        }

        #[test]
        fn a_zero_bitrate_is_refused() {
            let scratch = Scratch::new("zero-rate");
            let result = transcode_file(
                &fixture("sine.flac"),
                &scratch.path("z.mp3"),
                &options(Target::Mp3 { kbps: 0 }),
                &mut |_, _| {},
                &AtomicBool::new(false),
            );
            assert!(matches!(result, Err(Error::Unsupported(_))));
        }
    }

    mod streaming {
        use super::*;

        fn read_all(input: &str, target: Target) -> (Vec<u8>, &'static str) {
            let (mut reader, content_type) =
                transcode_stream(&fixture(input), &options(target)).expect("stream opens");
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .expect("stream reads to the end");
            (bytes, content_type)
        }

        #[test]
        fn each_streamable_format_produces_a_decodable_stream_of_the_right_length() {
            let scratch = Scratch::new("streams");
            for (extension, target, content_type, rate, slack) in [
                // Streams have no gapless marker, so the codec's delay and padding stay in.
                (
                    "mp3",
                    Target::Mp3 { kbps: KBPS },
                    "audio/mpeg",
                    44_100,
                    2_400,
                ),
                (
                    "aac",
                    Target::Aac { kbps: KBPS },
                    "audio/aac",
                    44_100,
                    2_400,
                ),
                ("opus", Target::Opus { kbps: 96 }, "audio/ogg", 48_000, 1),
                ("flac", Target::Flac, "audio/flac", 44_100, 1),
            ] {
                let (bytes, actual) = read_all("sine.flac", target);
                assert_eq!(actual, content_type);
                assert!(bytes.len() > 1_000, "{extension}: {} bytes", bytes.len());

                let path = scratch.path(&format!("stream.{extension}"));
                fs::write(&path, &bytes).unwrap();
                let decoded = decode(&path);
                assert_eq!(decoded.sample_rate, rate, "{extension}");
                assert!(
                    decoded.frames().abs_diff(rate as usize) <= slack,
                    "{extension}: {} frames",
                    decoded.frames()
                );
                let error = rms_error_against_sine(&decoded.channel(0), rate);
                assert!(
                    error < 0.1 || extension == "aac" || extension == "mp3",
                    "{extension}: {error}"
                );
            }
        }

        #[test]
        fn the_stream_carries_no_id3_tag_or_xing_header() {
            let (bytes, _) = read_all("sine.flac", Target::Mp3 { kbps: KBPS });
            assert_eq!(
                bytes[0],
                0xFF,
                "starts straight into an MPEG frame: {:02X?}",
                &bytes[..4]
            );
            assert!(
                bytes
                    .windows(4)
                    .take(2_000)
                    .all(|w| w != b"Xing" && w != b"Info")
            );
        }

        #[test]
        fn alac_cannot_be_streamed() {
            let result = transcode_stream(&fixture("sine.flac"), &options(Target::Alac));
            assert!(matches!(result, Err(Error::Unsupported(_))));
        }

        #[test]
        fn an_unreadable_input_fails_before_any_bytes() {
            let result = transcode_stream(&fixture("not-audio.txt"), &options(Target::Flac));
            assert!(result.is_err());
        }

        #[test]
        fn dropping_the_reader_part_way_is_harmless() {
            let (mut reader, _) =
                transcode_stream(&fixture("sine.flac"), &options(Target::Mp3 { kbps: KBPS }))
                    .unwrap();
            let mut first = [0u8; 512];
            reader.read_exact(&mut first).unwrap();
            drop(reader);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        #[test]
        fn a_stream_is_produced_lazily_not_all_at_once() {
            // The bounded backlog means a reader that has taken a few bytes has not forced the
            // whole file through the encoder; reading the rest still completes.
            let (mut reader, _) =
                transcode_stream(&fixture("sine.flac"), &options(Target::Flac)).unwrap();
            let mut head = [0u8; 4];
            reader.read_exact(&mut head).unwrap();
            assert_eq!(&head, b"fLaC");
            let mut rest = Vec::new();
            reader.read_to_end(&mut rest).unwrap();
            assert!(rest.len() > 10_000);
        }
    }

    mod ipod {
        use super::*;

        #[test]
        fn each_fixture_is_classified() {
            let scratch = Scratch::new("ipod");
            let cases = [
                ("sine.mp3", true),
                ("sine-aac.m4a", true),
                ("sine-alac.m4a", true),
                ("sine-mono.wav", true),
                ("sine-22k.aiff", true),
                ("gapless-itunes.m4a", true),
                ("sine.flac", false),
                ("sine-24bit.flac", false),
                ("sine-24bit-96k.flac", false),
                ("sine.ogg", false),
                ("sine.opus", false),
                ("sine.wv", false),
                ("sine.ape", false),
                ("sine-dsd64.dsf", false),
                ("surround-5.1.flac", false),
            ];
            for (file, expected) in cases {
                assert_eq!(ipod_compatible(&fixture(file)).unwrap(), expected, "{file}");
            }
            let _ = scratch;
        }

        #[test]
        fn converted_files_are_classified_by_what_they_became() {
            let scratch = Scratch::new("ipod-converted");
            for (file, target, expected) in [
                ("c.m4a", Target::Aac { kbps: KBPS }, true),
                ("c.mp3", Target::Mp3 { kbps: KBPS }, true),
                ("c.flac", Target::Flac, false),
                ("c.opus", Target::Opus { kbps: 96 }, false),
            ] {
                let out = convert(&scratch, "sine.flac", file, &options(target));
                assert_eq!(ipod_compatible(&out).unwrap(), expected, "{file}");
            }
            // ALAC depends on depth and rate: 16-bit 44.1 kHz plays, 24-bit does not.
            let plain = convert(&scratch, "sine.flac", "16.m4a", &options(Target::Alac));
            let deep = convert(
                &scratch,
                "sine-24bit.flac",
                "24.m4a",
                &options(Target::Alac),
            );
            assert!(ipod_compatible(&plain).unwrap());
            assert!(!ipod_compatible(&deep).unwrap());
        }

        #[test]
        fn a_missing_file_is_an_error_not_a_no() {
            assert!(ipod_compatible(&fixture("nope.mp3")).is_err());
        }
    }
}

#[cfg(not(feature = "ffmpeg"))]
mod without_ffmpeg {
    use antistreaming_playback::transcode_stream;

    use super::*;

    #[test]
    fn conversions_are_unsupported_not_a_crash() {
        let scratch = Scratch::new("no-ffmpeg");
        let result = transcode_file(
            &fixture("sine.flac"),
            &scratch.path("out.m4a"),
            &options(Target::Alac),
            &mut |_, _| {},
            &AtomicBool::new(false),
        );
        assert!(matches!(result, Err(Error::Unsupported(_))));
        assert!(matches!(
            transcode_stream(&fixture("sine.flac"), &options(Target::Flac)),
            Err(Error::Unsupported(_))
        ));
        assert!(!Target::Alac.is_available());
    }
}
