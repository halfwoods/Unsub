//! Inspect local videos before copying them unchanged to a classic iPod.
//!
//! This is a conservative compatibility check, not a transcoder. The caller separately checks
//! container encryption and verifies the file's identity between inspection and copying.

use std::path::Path;

/// Limits for the original video iPod, or the later Classic/video nano hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoProfile {
    LegacyVideo,
    Classic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "ffmpeg"), allow(dead_code))]
enum VideoCodec {
    H264,
    Mpeg4,
    Other,
}

#[derive(Clone, Debug)]
struct Audio {
    aac_lc: bool,
    rate: i32,
    channels: i32,
    bitrate: i64,
}

/// Metadata read from the actual streams. Empty tags remain empty; callers can supply a filename
/// fallback for the title. Bitrate describes the video stream, in decimal kilobits per second.
#[derive(Clone, Debug)]
pub struct VideoInfo {
    pub duration_ms: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub bitrate_kbps: u32,
    pub sample_rate: Option<u32>,
    codec: VideoCodec,
    profile: i32,
    level: i32,
    width: i32,
    height: i32,
    yuv420p: bool,
    interlaced: bool,
    bitrate: i64,
    frame_rate: (i32, i32),
    average_frame_rate: (i32, i32),
    audio: Option<Audio>,
}

impl VideoInfo {
    /// A user-facing reason this file needs conversion, or `None` when its declared format fits.
    /// Unknown format properties are rejected rather than guessed.
    pub fn incompatibility(&self, target: VideoProfile) -> Option<String> {
        let reason = self.reason(target)?;
        Some(format!(
            "{reason} Convert this file to an iPod-compatible MP4 first."
        ))
    }

    fn reason(&self, target: VideoProfile) -> Option<&'static str> {
        if self.duration_ms == 0 || self.duration_ms > u64::from(u32::MAX) {
            return Some("The video has an unknown or unsupported duration.");
        }
        let (width, height, bitrate) = match (self.codec, target) {
            (VideoCodec::H264, VideoProfile::LegacyVideo) => (320, 240, 768_000),
            (VideoCodec::Mpeg4, VideoProfile::LegacyVideo) => (480, 480, 2_500_000),
            (_, VideoProfile::Classic) => (640, 480, 2_500_000),
            _ => (0, 0, 0),
        };
        match self.codec {
            // H.264 Baseline (66) and Constrained Baseline (66 | 512).
            VideoCodec::H264 => {
                let level = if target == VideoProfile::LegacyVideo {
                    13
                } else {
                    30
                };
                if !matches!(self.profile, 66 | 578) || self.level <= 0 || self.level > level {
                    return Some("The H.264 profile or level is not supported by this iPod.");
                }
            }
            // MPEG-4 Part 2 Simple Profile is 0; Advanced Simple is deliberately excluded.
            VideoCodec::Mpeg4 if self.profile == 0 => {}
            _ => return Some("This iPod needs H.264 Baseline or MPEG-4 Simple Profile video."),
        }
        if self.width <= 0 || self.height <= 0 || self.width > width || self.height > height {
            return Some("The video dimensions exceed this iPod's limits or are unknown.");
        }
        if !self.yuv420p || self.interlaced {
            return Some("This iPod needs progressive 8-bit 4:2:0 video.");
        }
        if self.bitrate <= 0 || self.bitrate > bitrate {
            return Some("The video bitrate exceeds this iPod's limit or is unknown.");
        }
        if !valid_frame_rate(self.frame_rate) || !valid_frame_rate(self.average_frame_rate) {
            return Some("This iPod needs a known frame rate of 30 frames per second or less.");
        }
        if let Some(audio) = &self.audio
            && (!audio.aac_lc
                || audio.rate <= 0
                || audio.rate > 48_000
                || !(1..=2).contains(&audio.channels)
                || audio.bitrate <= 0
                || audio.bitrate > 160_000)
        {
            return Some("Audio must be AAC-LC, mono or stereo, at up to 48 kHz and 160 kbps.");
        }
        None
    }
}

fn valid_frame_rate((num, den): (i32, i32)) -> bool {
    num > 0 && den > 0 && i64::from(num) <= 30 * i64::from(den)
}

/// Inspect a regular local MP4/M4V/MOV file. FFmpeg uses only the local file protocol and MOV
/// demuxer; external movie data references are disabled. Probe work and the first decoded frame
/// are bounded. No executable, output file or network access is needed.
#[cfg(feature = "ffmpeg")]
pub fn inspect(path: &Path) -> Result<VideoInfo, String> {
    use std::time::{Duration, Instant};

    use ffmpeg_next::{Dictionary, codec, ffi, format, media};

    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "mp4" | "m4v" | "mov") {
        return Err("Choose an MP4, M4V or MOV video file.".into());
    }
    let path = path
        .canonicalize()
        .map_err(|e| format!("Cannot read this file: {e}"))?;
    if path.to_str().is_none_or(|s| s.contains('\0')) {
        return Err("This filename cannot be opened by the video reader.".into());
    }
    let meta = path
        .metadata()
        .map_err(|e| format!("Cannot read this file: {e}"))?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > u64::from(u32::MAX) {
        return Err("Choose a nonempty regular video file smaller than 4 GiB.".into());
    }
    crate::ffmpeg_source::init();
    let started = Instant::now();
    let mut options = Dictionary::new();
    options.set("protocol_whitelist", "file");
    options.set("format_whitelist", "mov");
    options.set("enable_drefs", "0");
    options.set("use_absolute_path", "0");
    options.set("probesize", "5242880");
    options.set("analyzeduration", "3000000");
    options.set("max_streams", "16");
    let mut input = format::input_with_interrupt_and_dictionary(
        &path,
        move || started.elapsed() > Duration::from_secs(10),
        options,
    )
    .map_err(|_| "Cannot read a valid MP4/M4V/MOV video from this file.".to_owned())?;
    if !input.format().name().split(',').any(|name| name == "mov") {
        return Err("The file is not an MP4/M4V/MOV container.".into());
    }

    let mut video = None;
    let mut audio = None;
    for stream in input.streams() {
        let parameters = stream.parameters();
        // SAFETY: Parameters retains the stream's owner; the immutable pointer lives through
        // this scope. Only scalar codec fields are copied, never pointers into FFmpeg storage.
        let p = unsafe { &*parameters.as_ptr() };
        match parameters.medium() {
            media::Type::Video if video.is_none() => {
                video = Some((stream.index(), parameters.clone(), VideoInfo {
                    duration_ms: u64::try_from(input.duration()).unwrap_or(0) / 1000,
                    title: String::new(),
                    artist: String::new(),
                    album: String::new(),
                    bitrate_kbps: u32::try_from(p.bit_rate / 1000).unwrap_or(0),
                    sample_rate: None,
                    codec: match parameters.id() {
                        codec::Id::H264 => VideoCodec::H264,
                        codec::Id::MPEG4 => VideoCodec::Mpeg4,
                        _ => VideoCodec::Other,
                    },
                    profile: p.profile,
                    level: p.level,
                    width: p.width,
                    height: p.height,
                    yuv420p: p.format == ffi::AVPixelFormat::AV_PIX_FMT_YUV420P as i32,
                    interlaced: !matches!(p.field_order, ffi::AVFieldOrder::AV_FIELD_UNKNOWN | ffi::AVFieldOrder::AV_FIELD_PROGRESSIVE),
                    bitrate: p.bit_rate,
                    frame_rate: (stream.rate().numerator(), stream.rate().denominator()),
                    average_frame_rate: (stream.avg_frame_rate().numerator(), stream.avg_frame_rate().denominator()),
                    audio: None,
                }));
            }
            media::Type::Audio if audio.is_none() => {
                audio = Some(Audio {
                    aac_lc: parameters.id() == codec::Id::AAC && p.profile == 1,
                    rate: p.sample_rate,
                    channels: p.ch_layout.nb_channels,
                    bitrate: p.bit_rate,
                });
            }
            _ => return Err("Choose a video with one video stream and at most one audio stream. Convert files with extra tracks first.".into()),
        }
    }
    let (stream_index, parameters, mut info) =
        video.ok_or_else(|| "This file has no video stream.".to_owned())?;
    info.sample_rate = audio
        .as_ref()
        .and_then(|a| u32::try_from(a.rate).ok())
        .filter(|rate| *rate > 0);
    info.audio = audio;
    let metadata = input.metadata();
    let tag = |key| {
        metadata
            .get(key)
            .unwrap_or("")
            .trim()
            .chars()
            .take(512)
            .collect()
    };
    info.title = tag("title");
    info.artist = tag("artist");
    info.album = tag("album");

    // Do not ask a decoder to allocate frames for files already rejected by their parameters.
    // A small first-frame check catches empty/truncated streams and confirms progressive video.
    if info.incompatibility(VideoProfile::Classic).is_none() {
        inspect_first_frame(&mut input, stream_index, parameters, started, &mut info)?;
    }
    Ok(info)
}

#[cfg(feature = "ffmpeg")]
fn inspect_first_frame(
    input: &mut ffmpeg_next::format::context::Input,
    stream_index: usize,
    parameters: ffmpeg_next::codec::Parameters,
    started: std::time::Instant,
    info: &mut VideoInfo,
) -> Result<(), String> {
    use ffmpeg_next::{Error, Packet, codec, format::Pixel, frame};
    let invalid = |_| {
        "The video stream is damaged or cannot be decoded. Convert the original file first."
            .to_owned()
    };
    let mut context = codec::context::Context::from_parameters(parameters)
        .map_err(invalid)?
        .decoder();
    context.set_threading(codec::threading::Config::count(1));
    context.check(codec::decoder::Check::EXPLODE);
    // SAFETY: the context is exclusively owned here and has not been opened. Bound a frame
    // even if a bitstream disagrees with the dimensions declared by its container.
    unsafe { (*context.as_mut_ptr()).max_pixels = 640 * 480 };
    let mut decoder = context.video().map_err(invalid)?;
    let mut bytes = 0_usize;
    let mut decoded = frame::Video::empty();
    for _ in 0..256 {
        if started.elapsed() > std::time::Duration::from_secs(10) || bytes > 8 * 1024 * 1024 {
            break;
        }
        let mut packet = Packet::empty();
        match packet.read(input) {
            Ok(()) => {
                bytes = bytes.saturating_add(packet.size());
                if packet.stream() != stream_index {
                    continue;
                }
                decoder.send_packet(&packet).map_err(invalid)?;
            }
            Err(Error::Eof) => decoder.send_eof().map_err(invalid)?,
            Err(error) => return Err(invalid(error)),
        }
        match decoder.receive_frame(&mut decoded) {
            Ok(()) => {
                if decoded.width() != info.width as u32 || decoded.height() != info.height as u32 {
                    return Err("The video dimensions do not match its headers. Convert the original file first.".into());
                }
                info.yuv420p = decoded.format() == Pixel::YUV420P;
                info.interlaced |= decoded.is_interlaced();
                return Ok(());
            }
            Err(Error::Other { errno }) if errno == ffmpeg_next::util::error::EAGAIN => {}
            Err(error) => return Err(invalid(error)),
        }
    }
    Err("The video could not be verified within the inspection limit. Convert the original file first.".into())
}

#[cfg(not(feature = "ffmpeg"))]
pub fn inspect(_path: &Path) -> Result<VideoInfo, String> {
    Err("This build does not include video inspection support.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic() -> VideoInfo {
        VideoInfo {
            duration_ms: 500,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            bitrate_kbps: 200,
            sample_rate: Some(44_100),
            codec: VideoCodec::H264,
            profile: 578,
            level: 13,
            width: 320,
            height: 240,
            yuv420p: true,
            interlaced: false,
            bitrate: 200_000,
            frame_rate: (30_000, 1001),
            average_frame_rate: (30_000, 1001),
            audio: Some(Audio {
                aac_lc: true,
                rate: 44_100,
                channels: 2,
                bitrate: 128_000,
            }),
        }
    }

    #[test]
    fn video_limits_differ_by_hardware_and_accept_exact_boundaries() {
        let mut v = basic();
        assert!(v.incompatibility(VideoProfile::LegacyVideo).is_none());
        v.width = 640;
        v.height = 480;
        v.level = 30;
        v.bitrate = 2_500_000;
        assert!(v.incompatibility(VideoProfile::Classic).is_none());
        assert!(v.incompatibility(VideoProfile::LegacyVideo).is_some());
        v.bitrate += 1;
        assert!(v.incompatibility(VideoProfile::Classic).is_some());
    }

    #[test]
    fn unknown_or_unsupported_video_and_audio_are_rejected() {
        let variants: &[fn(&mut VideoInfo)] = &[
            |v| v.codec = VideoCodec::Other,
            |v| v.profile = 100,
            |v| v.level = -99,
            |v| v.width = 0,
            |v| v.height = 720,
            |v| v.yuv420p = false,
            |v| v.interlaced = true,
            |v| v.bitrate = 0,
            |v| v.duration_ms = 0,
            |v| v.duration_ms = u64::from(u32::MAX) + 1,
            |v| v.frame_rate = (31, 1),
            |v| v.frame_rate = (0, 1),
            |v| v.average_frame_rate = (60, 1),
            |v| v.average_frame_rate = (1, 0),
            |v| v.audio.as_mut().unwrap().aac_lc = false,
            |v| v.audio.as_mut().unwrap().rate = 96_000,
            |v| v.audio.as_mut().unwrap().channels = 6,
            |v| v.audio.as_mut().unwrap().bitrate = 160_001,
            |v| v.audio.as_mut().unwrap().bitrate = 0,
        ];
        for change in variants {
            let mut v = basic();
            change(&mut v);
            assert!(v.incompatibility(VideoProfile::Classic).is_some(), "{v:?}");
        }
    }

    #[test]
    fn mpeg4_simple_and_silent_video_are_supported() {
        let mut v = basic();
        v.codec = VideoCodec::Mpeg4;
        v.profile = 0;
        v.width = 480;
        v.height = 480;
        v.audio = None;
        assert!(v.incompatibility(VideoProfile::LegacyVideo).is_none());
        v.profile = 15;
        assert!(v.incompatibility(VideoProfile::Classic).is_some());
    }

    #[cfg(feature = "ffmpeg")]
    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/video")
            .join(name)
    }

    #[test]
    #[cfg(feature = "ffmpeg")]
    fn video_probe_reads_real_streams_and_metadata() {
        let video = inspect(&fixture("baseline.mp4")).unwrap();
        assert_eq!(video.title, "Synthetic Video");
        assert_eq!(video.artist, "Unsub Tests");
        assert_eq!(video.album, "Generated Fixtures");
        assert!((490..=550).contains(&video.duration_ms));
        assert_eq!(video.sample_rate, Some(44_100));
        assert!(
            video.incompatibility(VideoProfile::LegacyVideo).is_none(),
            "{video:?}"
        );
        assert!(
            inspect(&fixture("mpeg4-simple.mp4"))
                .unwrap()
                .incompatibility(VideoProfile::LegacyVideo)
                .is_none()
        );
    }

    #[test]
    #[cfg(feature = "ffmpeg")]
    fn video_probe_rejects_incompatible_files_and_fake_extensions() {
        for name in ["high-profile.mp4", "hd.mp4", "alac-audio.mp4"] {
            let video = inspect(&fixture(name)).unwrap();
            assert!(
                video.incompatibility(VideoProfile::Classic).is_some(),
                "{name}"
            );
        }
        assert!(inspect(&fixture("not-mp4.mp4")).is_err());
        assert!(
            inspect(&fixture("extra-audio.mp4"))
                .unwrap_err()
                .contains("extra tracks")
        );
        assert!(
            inspect(&fixture("audio-only.mp4"))
                .unwrap_err()
                .contains("no video")
        );
        assert!(inspect(&fixture("missing.mp4")).is_err());
        assert!(inspect(&fixture("README.md")).is_err());
    }
}
