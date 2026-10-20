//! Fallback decoding through FFmpeg, for what symphonia cannot read: Opus, WavPack, Monkey's
//! Audio and DSD (which FFmpeg decimates to PCM at one eighth of the DSD bit rate).

use std::path::Path;
use std::sync::Once;

use ffmpeg_next::format::{self, Sample, sample::Type};
use ffmpeg_next::software::resampling;
use ffmpeg_next::util::error::EAGAIN;
use ffmpeg_next::{
    ChannelLayout as FfmpegLayout, Error as FfmpegError, Packet, codec, frame, log, media,
};

use crate::decoder::{Backend, Codec, Info, Source};
use crate::mix::ChannelLayout;
use crate::{Error, Result};

/// `AV_NOPTS_VALUE`: a timestamp the container did not provide.
const NO_TIMESTAMP: i64 = i64::MIN;

/// How many packets in a row may be unreadable before the input is given up on. A demuxer can
/// resync after a damaged one, but one that cannot would otherwise be asked again for ever.
const MAX_UNREADABLE_PACKETS: usize = 1_000;

/// Prepares FFmpeg's global state, once.
pub(crate) fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // Initialisation can only fail by a missing library, which opening reports anyway.
        let _ = ffmpeg_next::init();
        log::set_level(log::Level::Quiet);
    });
}

pub struct FfmpegSource {
    input: format::context::Input,
    decoder: codec::decoder::Audio,
    stream_index: usize,
    time_base: ffmpeg_next::Rational,
    /// Stream timestamp of the first frame; positions are measured from here.
    start_time: i64,
    info: Info,
    /// Built from the first decoded frame, so it matches exactly what the decoder produces.
    converter: Option<resampling::Context>,
    /// What `converter` converts from: sample format, channels, speaker mask and rate.
    converting: Option<(Sample, u16, u64, u32)>,
    /// Whether a change in the audio's format partway through is followed (a live stream's
    /// station switching rate) rather than being an error (a file that is not what it says).
    adaptive: bool,
    samples: Vec<f32>,
    /// Frame position a seek asked for; earlier frames are dropped since seeks land before it.
    skip_to: Option<u64>,
    /// Where the demuxer landed after a seek, for decoders whose frames carry no timestamps.
    landed: Option<i128>,
    sent_eof: bool,
    decoded_any: bool,
    seeked: bool,
}

impl FfmpegSource {
    pub fn open(path: &Path) -> Result<FfmpegSource> {
        init();
        FfmpegSource::from_input(format::input(path)?, false)
    }

    /// Decodes an already opened input. `adaptive` is for live streams, whose format can change.
    pub fn from_input(input: format::context::Input, adaptive: bool) -> Result<FfmpegSource> {
        let stream = input
            .streams()
            .best(media::Type::Audio)
            .ok_or(Error::NoAudioTrack)?;
        let decoder = codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .audio()?;

        let sample_rate = decoder.rate();
        let channels = decoder.channels();
        if sample_rate == 0 || channels == 0 {
            return Err(Error::Unsupported("unknown sample format"));
        }

        let time_base = stream.time_base();
        // Containers count the frames a decoder discards at the start (Opus pre-skip) in the
        // duration; listeners do not hear them.
        let skipped_ms = initial_padding(&decoder) * 1000 / u64::from(sample_rate);
        let duration_ms = stream_duration_ms(stream.duration(), time_base)
            .or_else(|| u64::try_from(input.duration()).ok().map(|us| us / 1000))
            .map(|ms| ms.saturating_sub(skipped_ms));
        let info = Info {
            codec: codec_of(decoder.id()),
            container: "ffmpeg",
            sample_rate,
            channels,
            layout: ChannelLayout::from_mask(speaker_mask(decoder.channel_layout()), channels),
            bit_depth: bit_depth(&decoder),
            duration_ms,
        };

        Ok(FfmpegSource {
            stream_index: stream.index(),
            start_time: match stream.start_time() {
                NO_TIMESTAMP => 0,
                start => start,
            },
            time_base,
            input,
            decoder,
            info,
            converter: None,
            converting: None,
            adaptive,
            samples: Vec::new(),
            skip_to: None,
            landed: None,
            sent_eof: false,
            decoded_any: false,
            seeked: false,
        })
    }

    /// Hands the decoder more input: the next packet of the audio stream, or the end marker.
    fn feed(&mut self) -> Result<()> {
        let mut packet = Packet::empty();
        let mut unreadable = 0;
        loop {
            match packet.read(&mut self.input) {
                Ok(()) => break,
                Err(FfmpegError::Eof) => {
                    self.decoder.send_eof()?;
                    self.sent_eof = true;
                    return Ok(());
                }
                Err(FfmpegError::InvalidData) if unreadable < MAX_UNREADABLE_PACKETS => {
                    unreadable += 1;
                }
                Err(error) => return Err(error.into()),
            }
        }
        if packet.stream() == self.stream_index {
            if self.skip_to.is_some() && self.landed.is_none() {
                self.landed = packet
                    .pts()
                    .map(|pts| self.stream_ticks_to_frames(pts - self.start_time));
            }
            match self.decoder.send_packet(&packet) {
                // A damaged packet costs a few milliseconds of audio, not the whole track.
                Ok(()) | Err(FfmpegError::InvalidData) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    /// Converts a decoded frame to interleaved f32 in `self.samples`.
    fn convert(&mut self, decoded: &frame::Audio) -> Result<()> {
        let layout = if decoded.channel_layout().is_empty() {
            FfmpegLayout::default(i32::from(decoded.channels()))
        } else {
            decoded.channel_layout()
        };
        let now = (
            decoded.format(),
            decoded.channels(),
            speaker_mask(layout),
            decoded.rate(),
        );
        // The first frame says what the audio really is, which a live stream's probing, done
        // ahead of where playback starts, can have got wrong.
        if self.adaptive && self.converting.is_none_or(|before| before != now) {
            self.converter = None;
            self.info.sample_rate = decoded.rate();
            self.info.channels = decoded.channels();
            self.info.layout = ChannelLayout::from_mask(speaker_mask(layout), decoded.channels());
        }
        self.converting = Some(now);
        let converter = match &mut self.converter {
            Some(converter) => converter,
            None => self.converter.insert(resampling::Context::get(
                decoded.format(),
                layout,
                decoded.rate(),
                Sample::F32(Type::Packed),
                layout,
                decoded.rate(),
            )?),
        };
        let mut converted = frame::Audio::empty();
        converter.run(decoded, &mut converted)?;
        self.samples.clear();
        self.samples.extend_from_slice(converted.plane::<f32>(0));
        Ok(())
    }

    /// Frames to drop from the front of a decoded frame to honour the last seek.
    fn frames_to_skip(&mut self, decoded: &frame::Audio, frames: usize) -> usize {
        let Some(target) = self.skip_to else {
            return 0;
        };
        // Negative while the container still counts decoder pre-roll (Opus) from before zero.
        let first = match decoded.timestamp() {
            Some(timestamp) => Some(self.stream_ticks_to_frames(timestamp - self.start_time)),
            None => self.landed,
        };
        let Some(first) = first else {
            // Nothing says where this frame is, so the seek can only be assumed exact.
            self.skip_to = None;
            return 0;
        };
        let skip = usize::try_from(i128::from(target) - first).unwrap_or(0);
        if skip < frames {
            self.skip_to = None;
        } else if let Some(landed) = &mut self.landed {
            *landed += frames as i128;
        }
        skip.min(frames)
    }

    fn stream_ticks_to_frames(&self, ticks: i64) -> i128 {
        let numerator = i128::from(self.time_base.numerator());
        let denominator = i128::from(self.time_base.denominator().max(1));
        i128::from(ticks) * numerator * i128::from(self.info.sample_rate) / denominator
    }

    fn end_of_stream(&self) -> Result<Option<&[f32]>> {
        if self.decoded_any || self.seeked {
            Ok(None)
        } else {
            Err(Error::Empty)
        }
    }
}

impl Source for FfmpegSource {
    fn backend(&self) -> Backend {
        Backend::Ffmpeg
    }

    fn info(&self) -> Info {
        self.info
    }

    fn read_chunk(&mut self) -> Result<Option<&[f32]>> {
        loop {
            let mut decoded = frame::Audio::empty();
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => {
                    self.decoded_any = true;
                    self.convert(&decoded)?;
                    let channels = usize::from(self.info.channels);
                    let frames = self.samples.len() / channels;
                    let skip = self.frames_to_skip(&decoded, frames);
                    if skip < frames {
                        return Ok(Some(&self.samples[skip * channels..]));
                    }
                }
                Err(FfmpegError::Other { errno: EAGAIN }) if !self.sent_eof => self.feed()?,
                Err(FfmpegError::Eof | FfmpegError::Other { errno: EAGAIN }) => {
                    return self.end_of_stream();
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn seek_to(&mut self, position_ms: u64) -> Result<u64> {
        let microseconds = i64::try_from(position_ms.saturating_mul(1000)).unwrap_or(i64::MAX);
        // `max_ts` = target makes the demuxer land at or before it, never after.
        self.input.seek(microseconds, ..microseconds)?;
        self.decoder.flush();
        self.sent_eof = false;
        self.converter = None;
        self.landed = None;
        self.seeked = true;
        self.skip_to = Some(position_ms * u64::from(self.info.sample_rate) / 1000);
        Ok(position_ms)
    }
}

fn codec_of(id: codec::Id) -> Codec {
    use codec::Id;
    match id {
        Id::OPUS => Codec::Opus,
        Id::WAVPACK => Codec::WavPack,
        Id::APE => Codec::Ape,
        Id::DSD_LSBF | Id::DSD_MSBF | Id::DSD_LSBF_PLANAR | Id::DSD_MSBF_PLANAR => Codec::Dsd,
        Id::MP3 => Codec::Mp3,
        Id::AAC => Codec::Aac,
        Id::ALAC => Codec::Alac,
        Id::FLAC => Codec::Flac,
        Id::VORBIS => Codec::Vorbis,
        _ => Codec::Other,
    }
}

fn stream_duration_ms(duration: i64, time_base: ffmpeg_next::Rational) -> Option<u64> {
    if duration == NO_TIMESTAMP || duration < 0 || time_base.denominator() <= 0 {
        return None;
    }
    let ms = i128::from(duration) * i128::from(time_base.numerator()) * 1000
        / i128::from(time_base.denominator());
    u64::try_from(ms).ok()
}

/// The speaker bit mask, or zero when the decoder only knows how many channels there are.
fn speaker_mask(layout: FfmpegLayout) -> u64 {
    use ffmpeg_next::ffi::{AVChannelLayout, AVChannelOrder};
    if layout.is_empty() {
        return 0;
    }
    let raw: AVChannelLayout = layout.into();
    if raw.order == AVChannelOrder::AV_CHANNEL_ORDER_NATIVE {
        // SAFETY: the mask is the active member of the union for a native-order layout.
        unsafe { raw.u.mask }
    } else {
        0
    }
}

/// Frames the decoder drops from the start of the stream.
fn initial_padding(decoder: &codec::decoder::Audio) -> u64 {
    // SAFETY: `as_ptr` points at the decoder's live AVCodecContext.
    let frames = unsafe { (*decoder.as_ptr()).initial_padding };
    u64::try_from(frames).unwrap_or(0)
}

/// Bits per sample of the coded audio; `None` for lossy codecs and 1-bit DSD.
fn bit_depth(decoder: &codec::decoder::Audio) -> Option<u32> {
    // SAFETY: `as_ptr` points at the decoder's live AVCodecContext.
    let bits = unsafe { (*decoder.as_ptr()).bits_per_raw_sample };
    u32::try_from(bits).ok().filter(|&bits| bits > 1)
}
