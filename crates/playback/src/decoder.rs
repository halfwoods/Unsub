use std::any::Any;
use std::fs::File;
use std::io::ErrorKind;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error as SymphoniaError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Duration, Time, TimeBase, Timestamp};

use crate::{Error, Result, mp4};

/// Pulls interleaved f32 PCM out of an audio file, independent of any output device.
///
/// A call that fails with [`Error::Panicked`] leaves the decoder in an unknown state; discard it.
pub struct Decoder {
    path: PathBuf,
    format: Box<dyn FormatReader>,
    codec: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: TimeBase,
    sample_rate: u32,
    channels: u16,
    duration_ms: Option<u64>,
    samples: Vec<f32>,
    /// Timestamp of the first real frame, for containers that leave encoder delay in the stream
    /// and say separately how much to discard (mp4). Zero when the codec trims it itself.
    origin_ts: i64,
    /// Timestamp where real audio ends, for containers that mark trailing padding.
    end_ts: Option<i64>,
    /// Accurate seeks can only land on a packet boundary; frames before this are discarded.
    skip_until: Option<Timestamp>,
    /// Nothing has been read or seeked yet.
    pristine: bool,
    decoded_any: bool,
    seeked: bool,
}

impl Decoder {
    pub fn open(path: &Path) -> Result<Decoder> {
        guarded(|| Decoder::open_with(path, |reader| reader))
    }

    /// `open` with a hook on the freshly probed reader, which lets tests substitute a faulty one.
    pub(crate) fn open_with(
        path: &Path,
        wrap: impl FnOnce(Box<dyn FormatReader>) -> Box<dyn FormatReader>,
    ) -> Result<Decoder> {
        let source = MediaSourceStream::new(Box::new(File::open(path)?), Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
            hint.with_extension(extension);
        }

        let format = symphonia::default::get_probe().probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(Error::NoAudioTrack)?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .ok_or(Error::NoAudioTrack)?;
        let codec = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())?;

        // The decoder knows the final parameters (e.g. mp3 learns them from the first frame).
        let params = codec.codec_params();
        let sample_rate = params
            .sample_rate
            .ok_or(Error::Unsupported("unknown sample rate"))?;
        let channels = params
            .channels
            .as_ref()
            .map(|channels| channels.count())
            .and_then(|count| u16::try_from(count).ok())
            .filter(|&count| count > 0)
            .ok_or(Error::Unsupported("unknown channel layout"))?;
        let time_base = track
            .time_base
            .or_else(|| TimeBase::try_from_recip(sample_rate))
            .ok_or(Error::Unsupported("unknown time base"))?;

        // mp3 (LAME header) and ogg trim their delay and padding inside symphonia; for mp4 it
        // only parses the edit list, so apply it here. A track symphonia did trim is left alone.
        let trim = (format.format_info().short_name == "isomp4" && track.delay.is_none())
            .then(|| mp4::read_trim(path, track.id))
            .flatten();
        let origin_ts = trim
            .as_ref()
            .map_or(0, |trim| i64::try_from(trim.start).unwrap_or(0));
        let end_ts = trim
            .as_ref()
            .and_then(|trim| i64::try_from(trim.length?).ok())
            .map(|length| origin_ts.saturating_add(length));

        let duration_ms = match trim.and_then(|trim| trim.length) {
            Some(ticks) => time_base.calc_duration(Duration::from(ticks)),
            None => track
                .num_frames
                .map(|frames| frames * 1000 / u64::from(sample_rate))
                .map(Time::from_millis_u64)
                .or_else(|| time_base.calc_duration(track.duration?)),
        }
        .and_then(|time| u64::try_from(time.as_millis()).ok());

        Ok(Decoder {
            track_id: track.id,
            path: path.to_path_buf(),
            format: wrap(format),
            codec,
            time_base,
            sample_rate,
            channels,
            duration_ms,
            samples: Vec::new(),
            origin_ts,
            end_ts,
            skip_until: None,
            pristine: true,
            decoded_any: false,
            seeked: false,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    /// Decodes the next packet. The slice is valid until the next call; `None` marks end of stream.
    pub fn next_chunk(&mut self) -> Result<Option<&[f32]>> {
        guarded(|| self.read_chunk())
    }

    /// Seeks to the frame at `position_ms` and returns the position actually landed on.
    pub fn seek(&mut self, position_ms: u64) -> Result<u64> {
        guarded(|| self.seek_to(position_ms))
    }

    fn read_chunk(&mut self) -> Result<Option<&[f32]>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return self.end_of_stream(),
                // A file cut short mid-packet is common; play what is there.
                Err(SymphoniaError::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => {
                    return self.end_of_stream();
                }
                Err(error) => return Err(error.into()),
            };
            self.pristine = false;
            if packet.track_id != self.track_id {
                continue;
            }

            match self.codec.decode(&packet) {
                Ok(decoded) => {
                    decoded.copy_to_vec_interleaved(&mut self.samples);
                    self.decoded_any = true;
                }
                // A damaged packet costs a few milliseconds of audio, not the whole track.
                Err(SymphoniaError::DecodeError(_) | SymphoniaError::IoError(_)) => continue,
                Err(error) => return Err(error.into()),
            }

            // Position of the packet's first frame; the decoder has already dropped any
            // encoder-delay frames it was told about.
            let first = packet
                .pts
                .get()
                .saturating_add_unsigned(packet.trim_start.get());
            let channels = usize::from(self.channels);
            let frames = self.samples.len() / channels;
            if frames == 0 {
                // Some codecs emit nothing for their first packet.
                continue;
            }
            let window_start = self
                .skip_until
                .map_or(self.origin_ts, |target| target.get().max(self.origin_ts));
            let skip = self.frames_between(first, window_start).min(frames);
            let keep = self
                .end_ts
                .map_or(frames, |end| self.frames_between(first, end).min(frames));

            if keep == 0 {
                // Everything from here on is padding.
                return self.end_of_stream();
            }
            if skip == frames {
                continue;
            }
            if skip >= keep {
                // The seek target is past the trimmed end.
                return self.end_of_stream();
            }
            self.skip_until = None;
            return Ok(Some(&self.samples[skip * channels..keep * channels]));
        }
    }

    fn seek_to(&mut self, position_ms: u64) -> Result<u64> {
        // symphonia's readers can lose their place when seeking after reads (mp4 once it has hit
        // the end, flac inside its buffer), so only a freshly opened reader is trusted to seek.
        if !self.pristine {
            *self = Decoder::open_with(&self.path, |reader| reader)?;
        }
        self.pristine = false;
        self.seeked = true;

        let position = self
            .time_base
            .calc_timestamp(Time::from_millis_u64(position_ms))
            .ok_or(SymphoniaError::SeekError(SeekErrorKind::OutOfRange))?;
        let seeked = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts: Timestamp::new(position.get().saturating_add(self.origin_ts)),
                track_id: self.track_id,
            },
        )?;
        self.codec.reset();
        self.skip_until = Some(seeked.required_ts);

        let landed = Timestamp::new(seeked.required_ts.get() - self.origin_ts);
        let landed = self.time_base.calc_time_saturating(landed);
        Ok(u64::try_from(landed.as_millis()).unwrap_or(0))
    }

    /// Running dry without ever producing audio means the body is unreadable, not that the track
    /// is over; after a seek, though, there may genuinely be nothing left.
    fn end_of_stream(&self) -> Result<Option<&[f32]>> {
        if self.decoded_any || self.seeked {
            Ok(None)
        } else {
            Err(Error::Empty)
        }
    }

    /// Whole frames from timestamp `from` to `to`, zero when `to` is not later.
    fn frames_between(&self, from: i64, to: i64) -> usize {
        let ticks = u128::from(u64::try_from(to.saturating_sub(from)).unwrap_or(0));
        let TimeBase { numer, denom } = self.time_base;
        let frames = ticks * u128::from(numer.get()) * u128::from(self.sample_rate)
            / u128::from(denom.get());
        usize::try_from(frames).unwrap_or(usize::MAX)
    }
}

/// symphonia can panic on malformed files. Containing that here turns a bad file into an error
/// for its track instead of ending the thread that decodes everything else.
fn guarded<T>(action: impl FnOnce() -> Result<T>) -> Result<T> {
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

/// A reader that panics on demand, for testing that panics are contained.
#[cfg(test)]
pub(crate) mod fault {
    use symphonia::core::formats::{FormatInfo, MediaInfo, SeekedTo, Track};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::Metadata;
    use symphonia::core::packet::Packet;

    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Fault {
        NextPacket,
        Seek,
    }

    /// Stands in for `Decoder::open` with the reader panicking while the file is being probed.
    pub fn open_panics(_: &Path) -> Result<Decoder> {
        let real = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sine.flac");
        guarded(|| Decoder::open_with(&real, |_| panic!("injected fault while probing")))
    }

    pub struct Faulty(pub Box<dyn FormatReader>, pub Fault);

    impl Faulty {
        pub fn wrap(fault: Fault) -> impl FnOnce(Box<dyn FormatReader>) -> Box<dyn FormatReader> {
            move |reader| Box::new(Faulty(reader, fault))
        }
    }

    impl FormatReader for Faulty {
        fn format_info(&self) -> &FormatInfo {
            self.0.format_info()
        }

        fn media_info(&self) -> &MediaInfo {
            self.0.media_info()
        }

        fn metadata(&mut self) -> Metadata<'_> {
            self.0.metadata()
        }

        fn seek(
            &mut self,
            mode: SeekMode,
            to: SeekTo,
        ) -> symphonia::core::errors::Result<SeekedTo> {
            assert!(self.1 != Fault::Seek, "injected fault while seeking");
            self.0.seek(mode, to)
        }

        fn tracks(&self) -> &[Track] {
            self.0.tracks()
        }

        fn next_packet(&mut self) -> symphonia::core::errors::Result<Option<Packet>> {
            assert!(self.1 != Fault::NextPacket, "injected fault while reading");
            self.0.next_packet()
        }

        fn into_inner<'s>(self: Box<Self>) -> MediaSourceStream<'s>
        where
            Self: 's,
        {
            self.0.into_inner()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fault::{Fault, Faulty};
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
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
        let opened =
            guarded(|| Decoder::open_with(&fixture("sine.flac"), |_| panic!("bad header")));
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
}
