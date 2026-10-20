//! Running a conversion: decode, bring to the output's rate and channels, encode.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use ffmpeg_next::Dictionary;
use ffmpeg_next::format::{self, context::StreamIo};

use super::encoder::{self, Encoder, Settings};
use super::{Target, TranscodeOptions, plan, tags};
use crate::decoder::guarded;
use crate::ffmpeg_source;
use crate::mix::Mixer;
use crate::resample::Resampler;
use crate::{Decoder, Error, Result};

/// Chunks of encoded output a stream may run ahead of its reader.
const STREAM_BACKLOG: usize = 4;

/// Small, so the first bytes of a stream do not wait for a long run of audio to be encoded.
const STREAM_BUFFER_BYTES: usize = 4096;

/// What a source becomes.
struct Plan {
    settings: Settings,
}

impl Plan {
    fn new(decoder: &Decoder, options: &TranscodeOptions) -> Plan {
        let target = options.target;
        let rate = plan::output_rate(decoder.sample_rate(), target, options.max_sample_rate);
        let channels = plan::output_channels(decoder.channels());
        let depth = plan::output_depth(
            decoder.bit_depth(),
            decoder.sample_rate(),
            options.max_bit_depth,
        );
        let unchanged = rate == decoder.sample_rate()
            && channels == decoder.channels()
            && decoder.bit_depth() == Some(depth);
        let lossless = matches!(target, Target::Alac | Target::Flac);
        Plan {
            settings: Settings {
                target,
                rate,
                channels,
                depth,
                dither: lossless && plan::needs_dither(depth, unchanged),
            },
        }
    }
}

fn check(options: &TranscodeOptions) -> Result<()> {
    ffmpeg_source::init();
    if let Target::Aac { kbps } | Target::Mp3 { kbps } | Target::Opus { kbps } = options.target
        && kbps == 0
    {
        return Err(Error::Unsupported("the bitrate must be above zero"));
    }
    if !encoder::is_available(options.target) {
        return Err(Error::Unsupported("this FFmpeg build lacks the encoder"));
    }
    Ok(())
}

pub fn to_file(
    input: &Path,
    output: &Path,
    options: &TranscodeOptions,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<()> {
    check(options)?;
    let temp = temp_path(output);
    let converted = guarded(|| convert_file(input, &temp, options, progress, cancel));
    match converted {
        Ok(()) => fs::rename(&temp, output).map_err(|error| {
            let _ = fs::remove_file(&temp);
            Error::from(error)
        }),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// The name the result is written under until it is complete. It keeps the real file name, since
/// tag libraries go by the extension, and its dot hides it from library scans.
fn temp_path(output: &Path) -> PathBuf {
    let name = output.file_name().unwrap_or_default().to_string_lossy();
    output.with_file_name(format!(".partial-{name}"))
}

fn convert_file(
    input: &Path,
    temp: &Path,
    options: &TranscodeOptions,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<()> {
    let mut decoder = Decoder::open(input)?;
    let plan = Plan::new(&decoder, options);
    let total_ms = decoder.duration_ms().unwrap_or(0);
    let muxer = encoder::muxer(options.target, false)
        .ok_or(Error::Unsupported("no container for this format"))?;

    let sink = format::output_as(temp, muxer)?;
    let mut encoder = Encoder::open(sink, plan.settings, Dictionary::new())?;
    run(&mut decoder, &plan, &mut encoder, &mut |done_ms| {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        progress(done_ms, total_ms);
        Ok(())
    })?;
    let gapless = encoder.finish()?;

    let tag = if options.keep_tags {
        tags::read_tag(input)
    } else {
        None
    };
    let marker = matches!(options.target, Target::Alac | Target::Aac { .. }).then_some(&gapless);
    tags::write_tags(temp, tag, marker)?;
    progress(total_ms, total_ms);
    Ok(())
}

/// Feeds everything `decoder` produces through `encoder`, calling `step` with the milliseconds
/// decoded so far after each chunk; an error from `step` stops the run.
fn run(
    decoder: &mut Decoder,
    plan: &Plan,
    encoder: &mut Encoder,
    step: &mut dyn FnMut(u64) -> Result<()>,
) -> Result<()> {
    let mixer = Mixer::new(decoder.channel_layout(), plan.settings.channels);
    let mut resampler = Resampler::new(decoder.sample_rate(), plan.settings.rate, mixer)?;
    let rate = u64::from(decoder.sample_rate());
    let channels = usize::from(decoder.channels());
    let mut converted = Vec::new();
    let mut decoded_frames = 0;

    while let Some(chunk) = decoder.next_chunk()? {
        decoded_frames += (chunk.len() / channels) as u64;
        converted.clear();
        resampler.push(chunk, &mut converted)?;
        encoder.push(&converted)?;
        step(decoded_frames * 1000 / rate)?;
    }
    converted.clear();
    resampler.finish(&mut converted)?;
    encoder.push(&converted)
}

pub fn to_stream(
    input: &Path,
    options: &TranscodeOptions,
) -> Result<(Box<dyn Read + Send>, &'static str)> {
    check(options)?;
    let content_type = encoder::content_type(options.target)
        .ok_or(Error::Unsupported("this format cannot be streamed"))?;
    let decoder = Decoder::open(input)?;
    let plan = Plan::new(&decoder, options);

    let (sender, receiver) = mpsc::sync_channel(STREAM_BACKLOG);
    thread::Builder::new()
        .name("unsub-transcode".into())
        .spawn(move || {
            let failures = sender.clone();
            if let Err(error) = guarded(|| stream(decoder, &plan, sender)) {
                let _ = failures.send(Err(io::Error::other(error.to_string())));
            }
        })?;
    Ok((Box::new(ChunkReader::new(receiver)), content_type))
}

fn stream(
    mut decoder: Decoder,
    plan: &Plan,
    sender: SyncSender<io::Result<Vec<u8>>>,
) -> Result<()> {
    let target = plan.settings.target;
    let muxer =
        encoder::muxer(target, true).ok_or(Error::Unsupported("this format cannot be streamed"))?;
    let io = StreamIo::from_write_with_capacity(ChunkWriter { sender }, STREAM_BUFFER_BYTES)?;
    let sink = format::output_to_stream(io, None, Some(muxer))?;

    // A stream cannot be rewound to fill in a header at the end, so ask for none, and for no
    // ID3 tag that a player would have to skip.
    let mut options = Dictionary::new();
    if matches!(target, Target::Mp3 { .. }) {
        options.set("write_xing", "0");
        options.set("id3v2_version", "0");
    }
    let mut encoder = Encoder::open(sink, plan.settings, options)?;
    run(&mut decoder, plan, &mut encoder, &mut |_| Ok(()))?;
    encoder.finish()?;
    Ok(())
}

/// Hands whatever FFmpeg writes to the reader, and fails once the reader has gone.
struct ChunkWriter {
    sender: SyncSender<io::Result<Vec<u8>>>,
}

impl Write for ChunkWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.sender
            .send(Ok(bytes.to_vec()))
            .map(|()| bytes.len())
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The reading end of a streamed conversion.
struct ChunkReader {
    receiver: Receiver<io::Result<Vec<u8>>>,
    chunk: Vec<u8>,
    offset: usize,
}

impl ChunkReader {
    fn new(receiver: Receiver<io::Result<Vec<u8>>>) -> Self {
        ChunkReader {
            receiver,
            chunk: Vec::new(),
            offset: 0,
        }
    }
}

impl Read for ChunkReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while self.offset >= self.chunk.len() {
            match self.receiver.recv() {
                Ok(chunk) => {
                    self.chunk = chunk?;
                    self.offset = 0;
                }
                // The writer finished and hung up: the stream is complete.
                Err(_) => return Ok(0),
            }
        }
        let count = (self.chunk.len() - self.offset).min(buffer.len());
        buffer[..count].copy_from_slice(&self.chunk[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}
