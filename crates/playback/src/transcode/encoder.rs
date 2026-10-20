//! Encoding and muxing through FFmpeg.

use ffmpeg_next::format::{self, Sample, sample::Type};
use ffmpeg_next::util::error::EAGAIN;
use ffmpeg_next::{
    ChannelLayout, Dictionary, Error as FfmpegError, Packet, Rational, codec, encoder, frame,
};

use super::Target;
use super::dither::Quantizer;
use super::tags::Smpb;
use crate::{Error, Result};

/// How the output is shaped, decided before the encoder opens.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub target: Target,
    pub rate: u32,
    pub channels: u16,
    /// Bits per sample, for the lossless targets.
    pub depth: u32,
    pub dither: bool,
}

/// The FFmpeg encoder for `target`.
fn encoder_name(target: Target) -> &'static str {
    match target {
        Target::Alac => "alac",
        Target::Aac { .. } => "aac",
        Target::Mp3 { .. } => "libmp3lame",
        Target::Opus { .. } => "libopus",
        Target::Flac => "flac",
    }
}

/// Whether this FFmpeg build can encode `target`.
pub fn is_available(target: Target) -> bool {
    encoder::find_by_name(encoder_name(target)).is_some()
}

/// The muxer for `target`: a seekable file, or a stream that can be written without seeking.
pub fn muxer(target: Target, streaming: bool) -> Option<&'static str> {
    Some(match (target, streaming) {
        (Target::Alac | Target::Aac { .. }, false) => "ipod",
        (Target::Aac { .. }, true) => "adts",
        (Target::Alac, true) => return None,
        (Target::Mp3 { .. }, _) => "mp3",
        (Target::Opus { .. }, _) => "opus",
        (Target::Flac, _) => "flac",
    })
}

/// The HTTP content type of `target` streamed, or `None` if it cannot be streamed.
pub fn content_type(target: Target) -> Option<&'static str> {
    Some(match target {
        Target::Alac => return None,
        Target::Aac { .. } => "audio/aac",
        Target::Mp3 { .. } => "audio/mpeg",
        Target::Opus { .. } => "audio/ogg",
        Target::Flac => "audio/flac",
    })
}

/// The sample layout each encoder takes.
fn sample_format(target: Target, depth: u32) -> Sample {
    match (target, depth <= 16) {
        (Target::Alac, true) => Sample::I16(Type::Planar),
        (Target::Alac, false) => Sample::I32(Type::Planar),
        (Target::Flac, true) => Sample::I16(Type::Packed),
        (Target::Flac, false) => Sample::I32(Type::Packed),
        (Target::Aac { .. } | Target::Mp3 { .. }, _) => Sample::F32(Type::Planar),
        (Target::Opus { .. }, _) => Sample::F32(Type::Packed),
    }
}

fn bit_rate(target: Target) -> Option<usize> {
    match target {
        Target::Aac { kbps } | Target::Mp3 { kbps } | Target::Opus { kbps } => {
            Some(kbps as usize * 1000)
        }
        Target::Alac | Target::Flac => None,
    }
}

pub struct Encoder {
    output: format::context::Output,
    encoder: encoder::Audio,
    settings: Settings,
    sample_format: Sample,
    layout: ChannelLayout,
    quantizer: Quantizer,
    /// Samples per frame the encoder insists on; zero when it takes any.
    frame_size: usize,
    /// The encoder's delay at the start of its output, in samples.
    priming: u64,
    stream_time_base: Rational,
    /// Interleaved samples waiting for a whole frame.
    pending: Vec<f32>,
    /// Frames sent to the encoder so far, per channel; also the next frame's timestamp.
    sent: u64,
    /// Samples in all the packets written so far.
    written: u64,
}

impl Encoder {
    /// Opens the encoder for `settings` and writes the container header to `output`.
    pub fn open(
        mut output: format::context::Output,
        settings: Settings,
        header_options: Dictionary,
    ) -> Result<Encoder> {
        let name = encoder_name(settings.target);
        let codec = encoder::find_by_name(name)
            .ok_or(Error::Unsupported("this FFmpeg build lacks the encoder"))?;
        let sample_format = sample_format(settings.target, settings.depth);
        let layout = match settings.channels {
            1 => ChannelLayout::MONO,
            _ => ChannelLayout::STEREO,
        };

        let mut context = codec::context::Context::new_with_codec(codec)
            .encoder()
            .audio()?;
        context.set_rate(settings.rate as i32);
        context.set_channel_layout(layout);
        context.set_format(sample_format);
        context.set_time_base((1, settings.rate as i32));
        if let Some(bits_per_second) = bit_rate(settings.target) {
            context.set_bit_rate(bits_per_second);
        } else {
            // SAFETY: `as_mut_ptr` points at the live, not yet opened, codec context.
            unsafe { (*context.as_mut_ptr()).bits_per_raw_sample = settings.depth as i32 };
        }
        if output
            .format()
            .flags()
            .contains(format::flag::Flags::GLOBAL_HEADER)
        {
            context.set_flags(codec::flag::Flags::GLOBAL_HEADER);
        }
        let encoder = context.open_as(codec)?;

        let mut stream = output.add_stream(codec)?;
        stream.set_parameters(&encoder);
        stream.set_time_base((1, settings.rate as i32));
        output.write_header_with(header_options)?;
        let stream_time_base = output
            .stream(0)
            .map(|stream| stream.time_base())
            .ok_or(Error::Unsupported("the muxer created no stream"))?;

        // SAFETY: `as_ptr` points at the live, opened codec context.
        let (frame_size, priming) = unsafe {
            let context = encoder.as_ptr();
            (
                usize::try_from((*context).frame_size).unwrap_or(0),
                u64::try_from((*context).initial_padding).unwrap_or(0),
            )
        };

        Ok(Encoder {
            output,
            encoder,
            settings,
            sample_format,
            layout,
            quantizer: Quantizer::new(settings.depth, settings.dither),
            frame_size,
            priming,
            stream_time_base,
            pending: Vec::new(),
            sent: 0,
            written: 0,
        })
    }

    /// Encodes `samples`, interleaved at the output's rate and channel count.
    pub fn push(&mut self, samples: &[f32]) -> Result<()> {
        self.pending.extend_from_slice(samples);
        let channels = usize::from(self.settings.channels);
        let whole = match self.frame_size {
            0 => self.pending.len() / channels * channels,
            size => size * channels,
        };
        let mut taken = 0;
        while whole > 0 && self.pending.len() - taken >= whole {
            let chunk = self.pending[taken..taken + whole].to_vec();
            self.encode(&chunk)?;
            taken += whole;
        }
        self.pending.drain(..taken);
        Ok(())
    }

    /// Flushes everything, writes the container trailer, and reports the gapless numbers.
    pub fn finish(mut self) -> Result<Smpb> {
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.encode(&rest)?;
        }
        self.encoder.send_eof()?;
        self.drain()?;
        self.output.write_trailer()?;

        let samples = self.sent;
        Ok(Smpb {
            priming: self.priming,
            padding: self.written.saturating_sub(self.priming + samples),
            samples,
        })
    }

    fn encode(&mut self, samples: &[f32]) -> Result<()> {
        let frame = self.build_frame(samples);
        self.sent += (samples.len() / usize::from(self.settings.channels)) as u64;
        self.encoder.send_frame(&frame)?;
        self.drain()
    }

    /// Moves every packet the encoder has ready into the container.
    fn drain(&mut self) -> Result<()> {
        let mut packet = Packet::empty();
        loop {
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    self.written += u64::try_from(packet.duration()).unwrap_or(0);
                    packet.set_stream(0);
                    packet.rescale_ts(
                        Rational::new(1, self.settings.rate as i32),
                        self.stream_time_base,
                    );
                    packet.write_interleaved(&mut self.output)?;
                }
                Err(FfmpegError::Eof | FfmpegError::Other { errno: EAGAIN }) => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Packs interleaved f32 samples into a frame in the encoder's own sample layout.
    fn build_frame(&mut self, samples: &[f32]) -> frame::Audio {
        let channels = usize::from(self.settings.channels);
        let frames = samples.len() / channels;
        let mut frame = frame::Audio::new(self.sample_format, frames, self.layout);
        frame.set_rate(self.settings.rate);
        frame.set_pts(Some(self.sent as i64));

        match self.sample_format {
            Sample::F32(Type::Planar) => {
                for channel in 0..channels {
                    let plane = frame.plane_mut::<f32>(channel);
                    for (index, out) in plane.iter_mut().take(frames).enumerate() {
                        *out = samples[index * channels + channel];
                    }
                }
            }
            Sample::F32(_) => {
                frame.plane_mut::<f32>(0)[..samples.len()].copy_from_slice(samples);
            }
            Sample::I16(Type::Planar) => {
                for channel in 0..channels {
                    let plane = frame.plane_mut::<i16>(channel);
                    for (index, out) in plane.iter_mut().take(frames).enumerate() {
                        *out = self.quantizer.quantize(samples[index * channels + channel]) as i16;
                    }
                }
            }
            Sample::I16(_) => {
                let plane = frame.plane_mut::<i16>(0);
                for (out, sample) in plane.iter_mut().zip(samples) {
                    *out = self.quantizer.quantize(*sample) as i16;
                }
            }
            Sample::I32(Type::Planar) => {
                for channel in 0..channels {
                    let plane = frame.plane_mut::<i32>(channel);
                    for (index, out) in plane.iter_mut().take(frames).enumerate() {
                        *out = self.left_justified(samples[index * channels + channel]);
                    }
                }
            }
            Sample::I32(_) => {
                let plane = frame.plane_mut::<i32>(0);
                for (out, sample) in plane.iter_mut().zip(samples) {
                    *out = self.left_justified(*sample);
                }
            }
            _ => {}
        }
        frame
    }

    /// FFmpeg takes narrower samples in the top bits of a 32-bit container.
    fn left_justified(&mut self, sample: f32) -> i32 {
        self.quantizer.quantize(sample) << (32 - self.settings.depth)
    }
}
