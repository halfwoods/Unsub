//! Live streams: audio from a reader that cannot seek and may stall, such as an internet radio
//! station. Reading blocks for as long as the network takes, so decoding runs on a thread of its
//! own and the engine only ever polls for what has been decoded.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use serde::{Deserialize, Serialize};
use symphonia::core::io::MediaSource;

use crate::decoder::{Info, Source, can_fall_back, guarded};
use crate::mix::{ChannelLayout, Mixer};
use crate::output::{Format, Sink};
use crate::resample::Resampler;
use crate::symphonia_source::SymphoniaSource;
use crate::{Error, Result};

/// What a live stream is said to be, from its content type or its address: a hint to get the
/// right decoder started, not a promise. A stream that turns out to be something else is decoded
/// as what it is, by whichever decoder can; `Unknown` is looked at before deciding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveFormat {
    #[serde(rename = "mp3")]
    Mp3,
    /// Raw AAC frames in ADTS headers, as Icecast and SHOUTcast send them.
    #[serde(rename = "aac-adts")]
    AacAdts,
    /// Ogg holding Vorbis or FLAC; Opus in Ogg is `Opus`.
    #[serde(rename = "ogg")]
    Ogg,
    #[serde(rename = "opus")]
    Opus,
    /// FLAC frames with no container.
    #[serde(rename = "flac")]
    Flac,
    /// An MPEG transport stream, such as an HLS segment with AAC inside.
    #[serde(rename = "mpegts")]
    MpegTs,
    /// Fragmented MP4.
    #[serde(rename = "fmp4")]
    Fmp4,
    #[serde(rename = "unknown")]
    Unknown,
}

impl LiveFormat {
    /// The file extension symphonia takes as a hint, for the formats it reads.
    fn extension(self) -> Option<&'static str> {
        match self {
            LiveFormat::Mp3 => Some("mp3"),
            LiveFormat::AacAdts => Some("aac"),
            LiveFormat::Ogg => Some("ogg"),
            LiveFormat::Flac => Some("flac"),
            _ => None,
        }
    }

    /// Formats only FFmpeg reads.
    fn needs_ffmpeg(self) -> bool {
        matches!(
            self,
            LiveFormat::Opus | LiveFormat::MpegTs | LiveFormat::Fmp4
        )
    }

    /// Formats whose first bytes leave no doubt, which a wrong hint is not allowed to override.
    /// MP3 and ADTS are not among them: their frames can be found anywhere in a stream.
    fn is_certain(self) -> bool {
        !matches!(
            self,
            LiveFormat::Mp3 | LiveFormat::AacAdts | LiveFormat::Unknown
        )
    }
}

/// What the decoding thread hands to the engine.
pub(crate) enum Message {
    /// The audio from here on is in this format. Sent before the first audio and again whenever
    /// the format changes.
    Format(Info),
    /// Interleaved samples in the last announced format.
    Pcm(Vec<f32>),
    /// Nothing more will come: the stream ended, or failed with this error.
    Ended(Option<Error>),
}

pub(crate) enum Polled {
    Ready(Message),
    Pending,
    /// The decoding thread is gone without saying why.
    Gone,
}

/// How much of a stream may go by without any audio coming out of it before it is given up on.
/// A healthy stream yields audio every few kilobytes. One that does not has changed into
/// something its decoder cannot follow, as an MP3 stream does when its sample rate changes: the
/// reader takes the rest for junk and searches it for ever. Better to end, with a reason, and
/// let the caller connect again, than to stay silent.
const MAX_BYTES_WITHOUT_AUDIO: usize = 256 * 1024;

const STOPPED_PRODUCING_AUDIO: &str =
    "the stream stopped producing audio, perhaps because it changed format";

/// How many decoded chunks the thread may run ahead before it waits, which stops it reading
/// the network too: a stream nobody is listening to is not downloaded.
const CHUNKS_AHEAD: usize = 64;

/// A stream being decoded on its own thread. Dropping it asks the thread to stop, which it does
/// as soon as the reader gives it back control; it is not waited for, since a reader stuck on a
/// dead connection could take as long as the network likes.
pub(crate) struct LiveDecoder {
    messages: Receiver<Message>,
    stop: Arc<AtomicBool>,
}

impl LiveDecoder {
    pub fn spawn(source: Box<dyn Read + Send>, hint: LiveFormat) -> Result<LiveDecoder> {
        let (sender, messages) = mpsc::sync_channel(CHUNKS_AHEAD);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        thread::Builder::new()
            .name("unsub-live".into())
            .spawn(move || run(source, hint, &sender, &flag))?;
        Ok(LiveDecoder { messages, stop })
    }

    pub fn poll(&self) -> Polled {
        match self.messages.try_recv() {
            Ok(message) => Polled::Ready(message),
            Err(TryRecvError::Empty) => Polled::Pending,
            Err(TryRecvError::Disconnected) => Polled::Gone,
        }
    }
}

impl Drop for LiveDecoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run(
    source: Box<dyn Read + Send>,
    hint: LiveFormat,
    messages: &SyncSender<Message>,
    stop: &AtomicBool,
) {
    let outcome = guarded(|| pump(source, hint, messages, stop));
    // The receiver is only gone when playback has been stopped, and then nobody wants the news.
    let _ = messages.send(Message::Ended(outcome.err()));
}

fn pump(
    source: Box<dyn Read + Send>,
    hint: LiveFormat,
    messages: &SyncSender<Message>,
    stop: &AtomicBool,
) -> Result<()> {
    let (mut decoder, tap) = open(source, hint)?;
    tap.watch();
    let mut announced: Option<Info> = None;
    while !stop.load(Ordering::Relaxed) {
        let chunk = match decoder.read_chunk() {
            Ok(Some(chunk)) => chunk,
            Ok(None) => return Ok(()),
            // The connection's own words say more than the decoder's, which only saw it fail.
            Err(error) => return Err(tap.failure().map_or(error, Error::Stream)),
        };
        tap.audio_decoded();
        let pcm = chunk.to_vec();
        let info = decoder.info();
        if announced.is_none_or(|before| audio_of(&before) != audio_of(&info)) {
            announced = Some(info);
            if messages.send(Message::Format(info)).is_err() {
                return Ok(());
            }
        }
        if messages.send(Message::Pcm(pcm)).is_err() {
            return Ok(());
        }
    }
    Ok(())
}

/// What of a stream's format matters to the output: a change in anything else (the codec, say)
/// is not announced.
fn audio_of(info: &Info) -> (u32, u16, ChannelLayout, Option<u32>) {
    (info.sample_rate, info.channels, info.layout, info.bit_depth)
}

/// Starts decoding `source` with the decoder its hint points to, or whichever can if that one
/// cannot. The hint gives way to what the first bytes plainly are: symphonia's readers would
/// take a transport stream mistaken for MP3 for audio of some kind and fail on it later.
fn open(source: Box<dyn Read + Send>, hint: LiveFormat) -> Result<(Box<dyn Source>, Tap)> {
    let tap = Tap::new(source);
    let sniffed = sniff(&tap)?;
    let hint = if hint == LiveFormat::Unknown || sniffed.is_certain() {
        sniffed
    } else {
        hint
    };
    let decoder: Box<dyn Source> = if hint.needs_ffmpeg() {
        open_ffmpeg(tap.replay())?
    } else {
        let live = SymphoniaSource::open_live(Box::new(Unseekable(tap.replay())), hint.extension());
        match live {
            Ok(decoder) => Box::new(decoder),
            // Not something symphonia reads after all (Opus inside Ogg, say): FFmpeg starts over
            // from the first byte.
            Err(error) if can_fall_back(&error) => open_ffmpeg(tap.replay()).map_err(|_| error)?,
            Err(error) => return Err(error),
        }
    };
    tap.forget();
    Ok((decoder, tap))
}

#[cfg(feature = "ffmpeg")]
fn open_ffmpeg(reader: Tap) -> Result<Box<dyn Source>> {
    use ffmpeg_next::Dictionary;
    use ffmpeg_next::format::{self, context::StreamIo};

    use crate::ffmpeg_source::{self, FfmpegSource};

    ffmpeg_source::init();
    let mut options = Dictionary::new();
    // A station should be playing within a moment of connecting, not after FFmpeg has watched
    // five seconds of it to be sure.
    options.set("analyzeduration", "1000000");
    let input = format::input_from_stream(StreamIo::from_read(reader)?, None, Some(options))?;
    Ok(Box::new(FfmpegSource::from_input(input, true)?))
}

#[cfg(not(feature = "ffmpeg"))]
fn open_ffmpeg(_: Tap) -> Result<Box<dyn Source>> {
    Err(Error::Unsupported("this stream needs the `ffmpeg` feature"))
}

/// What a stream is, from the first bytes of it.
fn sniff(tap: &Tap) -> Result<LiveFormat> {
    // Enough for a transport stream, which shows itself a packet in.
    let mut head = [0u8; 512];
    let mut filled = 0;
    let mut reader = tap.replay();
    while filled < head.len() {
        match reader.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    if filled == 0 {
        return Err(Error::Empty);
    }
    Ok(format_of(&head[..filled]))
}

fn format_of(head: &[u8]) -> LiveFormat {
    let at = |offset: usize, tag: &[u8]| head.get(offset..offset + tag.len()) == Some(tag);
    if at(0, b"OggS") {
        return if head.windows(8).any(|window| window == b"OpusHead") {
            LiveFormat::Opus
        } else {
            LiveFormat::Ogg
        };
    }
    if at(0, b"fLaC") {
        return LiveFormat::Flac;
    }
    if head.first() == Some(&0x47) && head.get(188) == Some(&0x47) {
        return LiveFormat::MpegTs;
    }
    if [b"ftyp", b"styp", b"moof", b"moov"]
        .iter()
        .any(|tag| at(4, *tag))
    {
        return LiveFormat::Fmp4;
    }
    // MPEG audio frames start with eleven set bits; layer 0 is not a layer, which is how ADTS
    // tells itself apart from MP3.
    if head.first() == Some(&0xFF) && head.get(1).is_some_and(|&byte| byte & 0xF6 == 0xF0) {
        return LiveFormat::AacAdts;
    }
    LiveFormat::Mp3
}

/// A reader that remembers what it has returned until told to forget, so that a second decoder
/// can start over from the first byte if the first one cannot read the stream.
#[derive(Clone)]
struct Tap {
    shared: Arc<Mutex<Tapped>>,
    /// How far into `Tapped::recorded` this reader has got.
    position: usize,
}

struct Tapped {
    source: Box<dyn Read + Send>,
    recorded: Vec<u8>,
    recording: bool,
    /// Whether to count the bytes read since audio last came out of the decoder.
    watching: bool,
    since_audio: usize,
    /// Why reading stopped, once it has: the source failed, or the count passed
    /// [`MAX_BYTES_WITHOUT_AUDIO`].
    failure: Option<String>,
}

impl Tap {
    fn new(source: Box<dyn Read + Send>) -> Tap {
        Tap {
            shared: Arc::new(Mutex::new(Tapped {
                source,
                recorded: Vec::new(),
                recording: true,
                watching: false,
                since_audio: 0,
                failure: None,
            })),
            position: 0,
        }
    }

    /// A reader of the same stream that begins again at the first byte.
    fn replay(&self) -> Tap {
        Tap {
            shared: Arc::clone(&self.shared),
            position: 0,
        }
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, Tapped> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stops remembering: the stream is being decoded, so nothing will start over.
    fn forget(&self) {
        let mut tapped = self.locked();
        tapped.recording = false;
        tapped.recorded = Vec::new();
    }

    /// Starts counting the bytes that go by without audio coming out of the decoder.
    fn watch(&self) {
        self.locked().watching = true;
    }

    fn audio_decoded(&self) {
        self.locked().since_audio = 0;
    }

    fn failure(&self) -> Option<String> {
        self.locked().failure.clone()
    }
}

impl Read for Tap {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut tapped = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(remembered) = tapped
            .recorded
            .get(self.position..)
            .filter(|r| !r.is_empty())
        {
            let count = remembered.len().min(buf.len());
            buf[..count].copy_from_slice(&remembered[..count]);
            self.position += count;
            return Ok(count);
        }
        if let Some(failure) = &tapped.failure {
            return Err(io::Error::other(failure.clone()));
        }
        let count = match tapped.source.read(buf) {
            Ok(count) => count,
            // An interrupted read is retried by whoever called, not the end of the stream.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
            Err(error) => {
                tapped.failure = Some(error.to_string());
                return Err(error);
            }
        };
        if tapped.watching {
            tapped.since_audio += count;
            if tapped.since_audio > MAX_BYTES_WITHOUT_AUDIO {
                let failure = STOPPED_PRODUCING_AUDIO.to_owned();
                tapped.failure = Some(failure.clone());
                return Err(io::Error::other(failure));
            }
        }
        if tapped.recording {
            tapped.recorded.extend_from_slice(&buf[..count]);
            self.position = tapped.recorded.len();
        }
        Ok(count)
    }
}

/// A reader as a source that cannot seek.
struct Unseekable<R>(R);

impl<R: Read> Read for Unseekable<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl<R> Seek for Unseekable<R> {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::other("a live stream cannot seek"))
    }
}

impl<R: Read + Send + Sync> MediaSource for Unseekable<R> {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Audio further behind the ring than this is a stream running dry.
const LOW_WATER_MS: u64 = 60;

/// What a call to [`LiveFeed::fill`] changed about the stream's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Unchanged,
    /// Running out of audio: playback should hold until there is enough again.
    Buffering,
    /// Enough audio again.
    Resumed,
}

/// A live stream on its way to the ring: decoded audio converted to the device format and
/// queued, with playback held back until enough of it has built up.
pub(crate) struct LiveFeed {
    decoder: LiveDecoder,
    /// The device's format.
    out: Format,
    exact: bool,
    /// The format of the audio now arriving.
    info: Info,
    resampler: Option<Resampler>,
    scratch: Vec<f32>,
    queue: Vec<f32>,
    /// Samples of `queue` already handed to the ring.
    queued: usize,
    /// Frames handed to the ring since the stream began, in the consumed counter's index space.
    produced: u64,
    /// False while waiting for enough audio to start, or to start again after running dry.
    playing: bool,
    /// Why the stream is over, once it is; what is queued still plays.
    ended: Option<String>,
    /// A change of format that the open output cannot follow, held until everything before it
    /// has been heard.
    reopen: Option<Info>,
}

impl LiveFeed {
    pub fn new(decoder: LiveDecoder, info: Info, out: Format, exact: bool) -> Result<LiveFeed> {
        let mut feed = LiveFeed {
            decoder,
            out,
            exact,
            info,
            resampler: None,
            scratch: Vec::new(),
            queue: Vec::new(),
            queued: 0,
            produced: 0,
            playing: false,
            ended: None,
            reopen: None,
        };
        feed.convert_from(info)?;
        Ok(feed)
    }

    /// The format of the audio now arriving.
    pub fn info(&self) -> &Info {
        &self.info
    }

    pub fn is_buffering(&self) -> bool {
        !self.playing
    }

    /// Takes in what has been decoded, up to twice `buffer_frames` ahead of the ring, and hands
    /// as much as fits to `sink`. Playback starts, and starts again after running dry, once
    /// `buffer_frames` are waiting. `consumed` is how much of what was handed over has been heard.
    pub fn fill(
        &mut self,
        sink: &mut impl Sink,
        consumed: u64,
        buffer_frames: usize,
    ) -> Result<Step> {
        self.pull(buffer_frames.saturating_mul(2))?;
        let mut step = Step::Unchanged;
        if !self.playing
            && (self.frames_queued() >= buffer_frames
                || self.ended.is_some()
                || self.reopen.is_some())
        {
            self.playing = true;
            step = Step::Resumed;
        }
        if self.playing {
            let pushed = sink.push(&self.queue[self.queued..]);
            self.queued += pushed;
            self.produced += (pushed / usize::from(self.out.channels)) as u64;
            self.compact();
            let ahead = self.produced.saturating_sub(consumed);
            let low_water = u64::from(self.out.sample_rate) * LOW_WATER_MS / 1000;
            if self.frames_queued() == 0
                && ahead < low_water
                && self.ended.is_none()
                && self.reopen.is_none()
            {
                self.playing = false;
                step = Step::Buffering;
            }
        }
        Ok(step)
    }

    /// Why the stream is over, once everything decoded has been heard.
    pub fn finished(&self, consumed: u64) -> Option<&str> {
        let heard = self.frames_queued() == 0 && consumed >= self.produced;
        self.ended.as_deref().filter(|_| heard)
    }

    /// A change of format the output must be reopened for, once everything before it was heard.
    pub fn take_reopen(&mut self, consumed: u64) -> Option<Info> {
        if self.frames_queued() == 0 && consumed >= self.produced {
            self.reopen.take()
        } else {
            None
        }
    }

    /// Points the stream at a newly opened output, with audio in `info`'s format arriving. What
    /// was queued for the old one is dropped, and playback holds until it has built up again.
    pub fn retarget(&mut self, out: Format, exact: bool, info: Info) -> Result<()> {
        self.out = out;
        self.exact = exact;
        self.queue.clear();
        self.queued = 0;
        self.produced = 0;
        self.playing = false;
        self.convert_from(info)
    }

    fn frames_queued(&self) -> usize {
        (self.queue.len() - self.queued) / usize::from(self.out.channels)
    }

    /// Drops what has been handed over once that is most of the queue, so it does not grow for
    /// as long as the station plays.
    fn compact(&mut self) {
        if self.queued == self.queue.len() {
            self.queue.clear();
            self.queued = 0;
        } else if self.queued >= self.queue.len() / 2 {
            self.queue.drain(..self.queued);
            self.queued = 0;
        }
    }

    fn pull(&mut self, limit_frames: usize) -> Result<()> {
        while self.ended.is_none() && self.reopen.is_none() && self.frames_queued() < limit_frames {
            match self.decoder.poll() {
                Polled::Pending => break,
                Polled::Gone => self.end(None)?,
                Polled::Ready(Message::Pcm(pcm)) => self.enqueue(&pcm)?,
                Polled::Ready(Message::Format(info)) => self.change_format(info)?,
                Polled::Ready(Message::Ended(error)) => self.end(error)?,
            }
        }
        Ok(())
    }

    fn end(&mut self, error: Option<Error>) -> Result<()> {
        self.flush_resampler()?;
        self.ended = Some(match error {
            None => "The stream ended.".to_owned(),
            Some(error) => error.to_string(),
        });
        Ok(())
    }

    fn enqueue(&mut self, pcm: &[f32]) -> Result<()> {
        match self.resampler.as_mut() {
            Some(resampler) => {
                self.scratch.clear();
                resampler.push(pcm, &mut self.scratch)?;
                self.queue.extend_from_slice(&self.scratch);
            }
            None => self.queue.extend_from_slice(pcm),
        }
        Ok(())
    }

    fn flush_resampler(&mut self) -> Result<()> {
        if let Some(resampler) = self.resampler.as_mut() {
            self.scratch.clear();
            resampler.finish(&mut self.scratch)?;
            self.queue.extend_from_slice(&self.scratch);
        }
        Ok(())
    }

    /// The stream's audio now arrives in `info`'s format. The open output follows it by
    /// resampling, unless it is bit-perfect, in which case it has to be reopened to match.
    fn change_format(&mut self, info: Info) -> Result<()> {
        self.flush_resampler()?;
        if self.exact && audio_of(&info) != audio_of(&self.info) {
            self.reopen = Some(info);
            return Ok(());
        }
        self.convert_from(info)
    }

    fn convert_from(&mut self, info: Info) -> Result<()> {
        let source = Format {
            sample_rate: info.sample_rate,
            channels: info.channels,
        };
        self.info = info;
        self.resampler = (source != self.out)
            .then(|| {
                let mixer = Mixer::new(info.layout, self.out.channels);
                Resampler::new(source.sample_rate, self.out.sample_rate, mixer)
            })
            .transpose()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    fn bytes(name: &str) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read(path).unwrap()
    }

    #[test]
    fn the_first_bytes_say_what_a_stream_is() {
        let cases = [
            ("sine.mp3", LiveFormat::Mp3),
            ("live-adts.aac", LiveFormat::AacAdts),
            ("sine.ogg", LiveFormat::Ogg),
            ("sine.opus", LiveFormat::Opus),
            ("sine.flac", LiveFormat::Flac),
            ("live-seg0.ts", LiveFormat::MpegTs),
            ("live-fragmented.mp4", LiveFormat::Fmp4),
            ("sine-aac.m4a", LiveFormat::Fmp4),
            // Anything else is left to symphonia's own probing, which looks further in.
            ("not-audio.txt", LiveFormat::Mp3),
        ];
        for (file, format) in cases {
            let data = bytes(file);
            assert_eq!(format_of(&data[..data.len().min(512)]), format, "{file}");
        }
    }

    #[test]
    fn only_unmistakable_formats_override_a_hint() {
        assert!(LiveFormat::Ogg.is_certain() && LiveFormat::MpegTs.is_certain());
        assert!(!LiveFormat::Mp3.is_certain() && !LiveFormat::AacAdts.is_certain());
        assert!(!LiveFormat::Unknown.is_certain());
    }

    #[test]
    fn formats_have_the_names_the_radio_side_uses() {
        for (format, name) in [
            (LiveFormat::Mp3, "mp3"),
            (LiveFormat::AacAdts, "aac-adts"),
            (LiveFormat::Ogg, "ogg"),
            (LiveFormat::Opus, "opus"),
            (LiveFormat::Flac, "flac"),
            (LiveFormat::MpegTs, "mpegts"),
            (LiveFormat::Fmp4, "fmp4"),
        ] {
            assert_eq!(serde_json::to_value(format).unwrap(), name);
            assert_eq!(
                serde_json::from_str::<LiveFormat>(&format!("\"{name}\"")).unwrap(),
                format
            );
        }
    }

    #[test]
    fn a_second_reader_replays_the_stream_from_the_start_then_carries_on() {
        let tap = Tap::new(Box::new(io::Cursor::new((0..=255u8).collect::<Vec<_>>())));
        let mut first = tap.replay();
        let mut head = [0u8; 100];
        first.read_exact(&mut head).unwrap();

        let mut second = tap.replay();
        let mut again = [0u8; 100];
        second.read_exact(&mut again).unwrap();
        assert_eq!(head, again);
        // Having caught up, it carries on from the source, and so does anything after it.
        let mut rest = Vec::new();
        second.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, (100..=255u8).collect::<Vec<_>>());
    }

    #[test]
    fn once_forgotten_nothing_is_replayed_and_the_stream_still_flows() {
        let tap = Tap::new(Box::new(io::Cursor::new((0..=255u8).collect::<Vec<_>>())));
        let mut reader = tap.replay();
        let mut head = [0u8; 50];
        reader.read_exact(&mut head).unwrap();
        tap.forget();

        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, (50..=255u8).collect::<Vec<_>>());
        assert!(tap.locked().recorded.is_empty());
    }

    #[test]
    fn a_stream_that_yields_no_audio_for_too_long_is_refused_further_reading() {
        let tap = Tap::new(Box::new(io::repeat(7)));
        tap.watch();
        let mut reader = tap.replay();
        let mut buf = vec![0u8; 64 * 1024];
        for _ in 0..(MAX_BYTES_WITHOUT_AUDIO / buf.len()) {
            reader.read_exact(&mut buf).unwrap();
        }
        // Audio coming out sets the count back to nothing.
        tap.audio_decoded();
        for _ in 0..(MAX_BYTES_WITHOUT_AUDIO / buf.len()) {
            reader.read_exact(&mut buf).unwrap();
        }
        assert_eq!(tap.failure(), None);

        let mut refused = false;
        for _ in 0..(MAX_BYTES_WITHOUT_AUDIO / buf.len() + 1) {
            refused |= reader.read_exact(&mut buf).is_err();
        }
        assert!(
            refused
                && tap
                    .failure()
                    .is_some_and(|why| why.contains("stopped producing"))
        );
        assert!(reader.read_exact(&mut buf).is_err(), "and stays refused");
    }

    /// How long a test waits for the decoder's thread: a loaded machine can keep it from running for
    /// a good while, but a hang should still end in a failure.
    const PATIENCE: Duration = Duration::from_secs(30);

    /// Polls `ready` until it holds, and fails with `what` if it never does.
    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !ready() {
            assert!(Instant::now() < deadline, "{what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// How far the decoder has read into a [`Fed`] connection, so a test can wait for it to have
    /// used everything sent instead of guessing how long that takes.
    #[derive(Default)]
    struct Reading {
        taken: AtomicUsize,
        waiting: AtomicBool,
    }

    impl Reading {
        /// Whether the decoder has taken all `sent` bytes and is waiting for more. Every chunk it
        /// could make of them has been handed on by then, and no more will come.
        fn caught_up(&self, sent: usize) -> bool {
            self.taken.load(Ordering::SeqCst) == sent && self.waiting.load(Ordering::SeqCst)
        }
    }

    /// A connection fed by hand: bytes arrive when the test sends them, and the stream ends
    /// when the sender is dropped.
    struct Fed {
        bytes: mpsc::Receiver<Vec<u8>>,
        unread: Vec<u8>,
        reading: Arc<Reading>,
    }

    impl Read for Fed {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.unread.is_empty() {
                self.reading.waiting.store(true, Ordering::SeqCst);
                let more = self.bytes.recv();
                self.reading.waiting.store(false, Ordering::SeqCst);
                match more {
                    Ok(more) => self.unread = more,
                    Err(_) => return Ok(0),
                }
            }
            let count = self.unread.len().min(buf.len());
            buf[..count].copy_from_slice(&self.unread[..count]);
            self.unread.drain(..count);
            self.reading.taken.fetch_add(count, Ordering::SeqCst);
            Ok(count)
        }
    }

    /// A ring that holds `capacity` samples and is emptied only when the test says.
    struct Ring {
        samples: Vec<f32>,
        capacity: usize,
    }

    impl Sink for Ring {
        fn push(&mut self, samples: &[f32]) -> usize {
            let count = (self.capacity - self.samples.len()).min(samples.len()) / 2 * 2;
            self.samples.extend_from_slice(&samples[..count]);
            count
        }
    }

    /// Calls `fill` until something other than "unchanged" happens, or time runs out.
    fn until_step(feed: &mut LiveFeed, ring: &mut Ring, consumed: u64, buffer: usize) -> Step {
        let mut step = Step::Unchanged;
        wait_for("nothing happened", || {
            step = feed.fill(ring, consumed, buffer).unwrap();
            step != Step::Unchanged
        });
        step
    }

    #[test]
    fn playback_waits_for_the_buffer_holds_when_dry_and_finishes_when_the_stream_does() {
        let data = bytes("sine.mp3");
        let (sender, receiver) = mpsc::channel();
        let reading = Arc::new(Reading::default());
        let fed = Fed {
            bytes: receiver,
            unread: Vec::new(),
            reading: Arc::clone(&reading),
        };
        let decoder = LiveDecoder::spawn(Box::new(fed), LiveFormat::Mp3).unwrap();
        // Enough to start the decoder, which has to see a few frames before it knows the format.
        let head = 4_000;
        sender.send(data[..head].to_vec()).unwrap();
        let mut format = None;
        wait_for("the stream's format never came", || {
            match decoder.poll() {
                Polled::Ready(Message::Format(info)) => format = Some(info),
                Polled::Pending => {}
                _ => panic!("expected the format first"),
            }
            format.is_some()
        });
        let info = format.unwrap();
        // The decoder may be part-way through those bytes. Everything it makes of them has to be
        // on its way before the feed is looked at, or the rest could turn up after the stream
        // seems to have run dry.
        wait_for("the decoder never used the first bytes", || {
            reading.caught_up(head)
        });
        let out = Format {
            sample_rate: info.sample_rate,
            channels: info.channels,
        };
        let mut feed = LiveFeed::new(decoder, info, out, false).unwrap();
        let mut ring = Ring {
            samples: Vec::new(),
            capacity: 2 * 44_100,
        };
        let buffer = 8_820;

        // Held back until 0.2 s are waiting, then let go.
        assert!(feed.is_buffering());
        assert_eq!(until_step(&mut feed, &mut ring, 0, buffer), Step::Resumed);
        assert!(!feed.is_buffering() && !ring.samples.is_empty());

        // Everything handed over is heard, nothing is waiting, and no more has arrived.
        let heard = (ring.samples.len() / 2) as u64;
        assert_eq!(
            until_step(&mut feed, &mut ring, heard, buffer),
            Step::Buffering
        );
        assert!(feed.is_buffering());
        assert_eq!(feed.finished(heard), None, "still running, only dry");

        // The rest arrives and the stream ends: it plays out and then it is over.
        sender.send(data[head..].to_vec()).unwrap();
        drop(sender);
        assert_eq!(
            until_step(&mut feed, &mut ring, heard, buffer),
            Step::Resumed
        );
        wait_for("the stream never finished", || {
            feed.fill(&mut ring, heard, buffer).unwrap();
            feed.finished(u64::MAX).is_some()
        });
        assert_eq!(feed.finished(u64::MAX), Some("The stream ended."));
        assert_eq!(feed.finished(0), None, "not until it has all been heard");
    }
}
