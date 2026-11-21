use std::any::Any;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use symphonia::core::errors::Error as SymphoniaError;

use crate::mix::ChannelLayout;
use crate::symphonia_source::SymphoniaSource;
use crate::{Error, Result};

/// The codec a file's audio is stored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Mp3,
    Aac,
    Alac,
    Flac,
    Vorbis,
    Opus,
    WavPack,
    /// Monkey's Audio.
    Ape,
    Dsd,
    /// Uncompressed PCM, as in WAV and AIFF.
    Pcm,
    Other,
}

/// What a source knows about its stream once opened.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Info {
    pub codec: Codec,
    /// The container format: "isomp4", "wav", "aiff", "flac", "ogg", ... or "ffmpeg" for files
    /// FFmpeg reads.
    pub container: &'static str,
    pub sample_rate: u32,
    pub channels: u16,
    pub layout: ChannelLayout,
    /// Bits per sample in the file; `None` for lossy codecs, which have no integer depth.
    pub bit_depth: Option<u32>,
    pub duration_ms: Option<u64>,
}

/// Which decoder is reading a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Symphonia,
    Ffmpeg,
}

/// One way of turning a file into PCM.
pub(crate) trait Source: Send {
    fn backend(&self) -> Backend;
    fn info(&self) -> Info;
    fn read_chunk(&mut self) -> Result<Option<&[f32]>>;
    fn seek_to(&mut self, position_ms: u64) -> Result<u64>;
}

/// Pulls interleaved f32 PCM out of an audio file, independent of any output device.
///
/// Files are read by symphonia where it can, and otherwise (Opus, WavPack, APE, DSD) by FFmpeg
/// when the `ffmpeg` feature is on. A call that fails with [`Error::Panicked`] leaves the decoder
/// in an unknown state; discard it.
pub struct Decoder {
    source: Box<dyn Source>,
    info: Info,
    range: Option<(u64, u64)>,
    next_frame: u64,
    skip_frames: u64,
    range_buffer: Vec<f32>,
}

impl Decoder {
    pub fn open(path: &Path) -> Result<Decoder> {
        if is_ffmpeg_only(path) {
            return open_with_ffmpeg(path);
        }
        let opened = guarded(|| SymphoniaSource::open(path).map(Decoder::new));
        match opened {
            Err(error) if can_fall_back(&error) => open_fallback(path, error),
            other => other,
        }
    }

    /// `open` with symphonia only and a hook on the freshly probed reader, which lets tests
    /// substitute a faulty one.
    #[cfg(test)]
    pub(crate) fn open_with(
        path: &Path,
        wrap: impl FnOnce(
            Box<dyn symphonia::core::formats::FormatReader>,
        ) -> Box<dyn symphonia::core::formats::FormatReader>,
    ) -> Result<Decoder> {
        guarded(|| SymphoniaSource::open_with(path, wrap).map(Decoder::new))
    }

    fn new(source: impl Source + 'static) -> Decoder {
        Decoder {
            info: source.info(),
            source: Box::new(source),
            range: None,
            next_frame: 0,
            skip_frames: 0,
            range_buffer: Vec::new(),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.info.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.info.channels
    }

    /// Which decoder reads this file.
    pub fn backend(&self) -> Backend {
        self.source.backend()
    }

    /// The codec the audio is stored in.
    pub fn codec(&self) -> Codec {
        self.info.codec
    }

    /// The container format, e.g. "isomp4" for `.m4a`, or "ffmpeg" for files FFmpeg reads.
    pub fn container(&self) -> &'static str {
        self.info.container
    }

    /// Which speakers the channels are, which decides how they are downmixed.
    pub fn channel_layout(&self) -> ChannelLayout {
        self.info.layout
    }

    /// Bits per sample in the file, `None` for lossy formats.
    pub fn bit_depth(&self) -> Option<u32> {
        self.info.bit_depth
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.info.duration_ms
    }

    /// Decodes the next packet. The slice is valid until the next call; `None` marks end of stream.
    pub fn next_chunk(&mut self) -> Result<Option<&[f32]>> {
        let Some((_, end)) = self.range else { return guarded(|| self.source.read_chunk()) };
        let end_frame = end.saturating_mul(u64::from(self.info.sample_rate)) / 1000;
        let channels = usize::from(self.info.channels);
        while self.next_frame < end_frame {
            let Some(chunk) = guarded(|| self.source.read_chunk())? else { return Ok(None) };
            let frames = chunk.len() / channels;
            if frames == 0 { continue; }
            let skip = frames.min(usize::try_from(self.skip_frames).unwrap_or(usize::MAX));
            self.skip_frames -= skip as u64;
            self.next_frame += skip as u64;
            if skip == frames { continue; }
            let available = (frames - skip).min(end_frame.saturating_sub(self.next_frame) as usize);
            self.range_buffer.clear();
            self.range_buffer.extend_from_slice(&chunk[skip * channels..(skip + available) * channels]);
            self.next_frame += available as u64;
            return Ok(Some(&self.range_buffer));
        }
        Ok(None)
    }

    /// Restrict playback to original-file timestamps, including seeks.
    pub(crate) fn set_play_range(&mut self, mut range: Option<(u64, u64)>) -> Result<u64> {
        if range.is_some() && (self.info.sample_rate == 0 || self.info.channels == 0) {
            return Err(Error::Unsupported("invalid audio format for playback range"));
        }
        // Library metadata and the decoder can disagree slightly about a compressed file's end.
        if let (Some((start, end)), Some(duration)) = (range, self.info.duration_ms)
            && end > duration && end.saturating_sub(duration) <= 1000
        {
            range = Some((start, duration));
        }
        if range.is_some_and(|(start, end)| start >= end || self.info.duration_ms.is_some_and(|duration| end > duration)) {
            return Err(Error::Unsupported("invalid playback range"));
        }
        self.range = range;
        self.next_frame = 0;
        self.skip_frames = range.map_or(0, |(start, _)| start.saturating_mul(u64::from(self.info.sample_rate)) / 1000);
        Ok(range.map_or(0, |(start, _)| start))
    }

    /// Seeks to the frame at `position_ms` and returns the position actually landed on.
    pub fn seek(&mut self, position_ms: u64) -> Result<u64> {
        if let Some((start, end)) = self.range {
            let target = position_ms.clamp(start, end.saturating_sub(1));
            // Re-decode from the beginning. Some lossy decoders seek a few samples away from
            // their reported timestamp, which would make the preview differ from a permanent cut.
            guarded(|| self.source.seek_to(0))?;
            self.next_frame = 0;
            self.skip_frames = target.saturating_mul(u64::from(self.info.sample_rate)) / 1000;
            Ok(target)
        } else {
            guarded(|| self.source.seek_to(position_ms))
        }
    }
}

/// Whether symphonia failing with `error` means it cannot read this kind of file, as opposed to
/// the file being unreachable.
pub(crate) fn can_fall_back(error: &Error) -> bool {
    matches!(
        error,
        Error::NoAudioTrack | Error::Unsupported(_) | Error::Panicked(_) | Error::Decode(_)
    ) && !matches!(error, Error::Decode(SymphoniaError::IoError(_)))
}

fn open_fallback(path: &Path, symphonia_error: Error) -> Result<Decoder> {
    // symphonia's error is the one worth showing: it names what was wrong with the file.
    open_with_ffmpeg(path).map_err(|_| symphonia_error)
}

#[cfg(feature = "ffmpeg")]
fn open_with_ffmpeg(path: &Path) -> Result<Decoder> {
    use crate::ffmpeg_source::FfmpegSource;
    guarded(|| FfmpegSource::open(path).map(Decoder::new))
}

#[cfg(not(feature = "ffmpeg"))]
fn open_with_ffmpeg(_: &Path) -> Result<Decoder> {
    Err(Error::Unsupported("this format needs the `ffmpeg` feature"))
}

/// Whether the file is a format only FFmpeg can decode. symphonia's probe cannot be asked first
/// for these: it has no reader for them, so its MP3 reader may latch onto a chance byte pattern
/// and play noise.
fn is_ffmpeg_only(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut header = [0; 64];
    if !read_header(&mut file, &mut header) {
        return false;
    }
    // A leading ID3v2 tag pushes the real header past it (its size is a 28-bit syncsafe int).
    if header.starts_with(b"ID3") {
        let size = header[6..10]
            .iter()
            .fold(0u64, |size, byte| (size << 7) | u64::from(byte & 0x7F));
        if file.seek(SeekFrom::Start(10 + size)).is_err() || !read_header(&mut file, &mut header) {
            return false;
        }
    }
    header.starts_with(b"MAC ")
        || header.starts_with(b"wvpk")
        || header.starts_with(b"DSD ")
        || (header.starts_with(b"FRM8") && &header[12..16] == b"DSD ")
        || (header.starts_with(b"OggS") && header.windows(8).any(|window| window == b"OpusHead"))
}

fn read_header(file: &mut File, header: &mut [u8; 64]) -> bool {
    header.fill(0);
    file.read(header).is_ok_and(|read| read >= 16)
}

/// symphonia and FFmpeg can panic on malformed files. Containing that here turns a bad file into
/// an error for its track instead of ending the thread that decodes everything else.
pub(crate) fn guarded<T>(action: impl FnOnce() -> Result<T>) -> Result<T> {
    panic::catch_unwind(AssertUnwindSafe(action))
        .unwrap_or_else(|payload| Err(Error::Panicked(panic_message(&*payload))))
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::symphonia_source::fault::{Fault, Faulty};

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn playback_range_limits_samples_and_clamps_seeks() {
        for name in ["sine.flac", "sine.mp3", "sine-aac.m4a"] {
            let path = fixture(name);
            let mut original = Decoder::open(&path).unwrap();
            let channels = usize::from(original.channels());
            let rate = u64::from(original.sample_rate());
            let mut full = Vec::new();
            while let Some(chunk) = original.next_chunk().unwrap() { full.extend_from_slice(chunk); }

            let mut clipped = Decoder::open(&path).unwrap();
            clipped.set_play_range(Some((200, 700))).unwrap();
            let mut samples = Vec::new();
            while let Some(chunk) = clipped.next_chunk().unwrap() { samples.extend_from_slice(chunk); }
            let expected = (rate / 2) as usize * channels;
            assert_eq!(samples.len(), expected, "{name}");
            let offset = (rate / 5) as usize * channels;
            for (a, b) in samples.iter().zip(&full[offset..offset + samples.len()]) {
                assert!((a - b).abs() < 0.02, "wrong samples for {name}: {a} vs {b}");
            }
            assert!(clipped.seek(0).unwrap() >= 199);
            assert!(clipped.seek(900).unwrap() < 700);
            assert_eq!(clipped.seek(500).unwrap(), 500);
            let mut tail = Vec::new();
            while let Some(chunk) = clipped.next_chunk().unwrap() { tail.extend_from_slice(chunk); }
            assert_eq!(tail.len(), (rate / 5) as usize * channels, "{name}");
            let tail_offset = (rate / 2) as usize * channels;
            for (a, b) in tail.iter().zip(&full[tail_offset..tail_offset + tail.len()]) {
                assert!((a - b).abs() < 0.02, "wrong samples after seek for {name}");
            }
        }
    }

    #[test]
    fn invalid_playback_ranges_are_rejected() {
        let mut decoder = Decoder::open(&fixture("sine.flac")).unwrap();
        assert!(decoder.set_play_range(Some((500, 500))).is_err());
        assert!(decoder.set_play_range(Some((600, 500))).is_err());
        assert!(decoder.set_play_range(Some((0, u64::MAX))).is_err());
    }

    #[test]
    fn a_panic_becomes_an_error_carrying_its_message() {
        let literal: Result<()> = guarded(|| panic!("probe exploded"));
        assert!(matches!(literal, Err(Error::Panicked(message)) if message == "probe exploded"));

        let formatted: Result<()> = guarded(|| panic!("bad frame {}", 7));
        assert!(matches!(formatted, Err(Error::Panicked(message)) if message == "bad frame 7"));
    }

    #[test]
    fn results_pass_through_the_guard_unchanged() {
        assert_eq!(guarded(|| Ok(3)).unwrap(), 3);
        assert!(matches!(
            guarded::<()>(|| Err(Error::Empty)),
            Err(Error::Empty)
        ));
    }

    #[test]
    fn a_panic_while_opening_is_contained() {
        let opened = Decoder::open_with(&fixture("sine.flac"), |_| panic!("bad header"));
        assert!(matches!(opened, Err(Error::Panicked(_))));
    }

    #[test]
    fn a_panic_while_reading_is_contained_and_reported_for_that_decoder() {
        let mut decoder =
            Decoder::open_with(&fixture("sine.flac"), Faulty::wrap(Fault::NextPacket)).unwrap();
        let outcome = decoder.next_chunk().map(|chunk| chunk.map(<[f32]>::len));
        assert!(matches!(outcome, Err(Error::Panicked(message)) if message.contains("reading")));
    }

    #[test]
    fn a_panic_while_seeking_is_contained() {
        let mut decoder =
            Decoder::open_with(&fixture("sine.flac"), Faulty::wrap(Fault::Seek)).unwrap();
        assert!(matches!(decoder.seek(500), Err(Error::Panicked(_))));
    }

    #[test]
    fn only_unreadable_formats_fall_back_not_unreachable_files() {
        assert!(can_fall_back(&Error::NoAudioTrack));
        assert!(can_fall_back(&Error::Unsupported("codec")));
        assert!(can_fall_back(&Error::Panicked(String::new())));
        assert!(can_fall_back(&Error::Decode(SymphoniaError::Unsupported(
            "probe"
        ))));
        assert!(!can_fall_back(&Error::Io(
            std::io::ErrorKind::NotFound.into()
        )));
        assert!(!can_fall_back(&Error::Empty));
    }
}
