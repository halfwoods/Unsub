use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use symphonia::core::audio::Channels;
use symphonia::core::codecs::audio::well_known::CODEC_ID_ALAC;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error as SymphoniaError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Duration, Time, TimeBase, Timestamp};

use crate::decoder::{Backend, Info, Source};
use crate::mix::ChannelLayout;
use crate::{Error, Result, mp4};

/// Decodes everything symphonia can read.
pub struct SymphoniaSource {
    path: PathBuf,
    format: Box<dyn FormatReader>,
    codec: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: TimeBase,
    info: Info,
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

impl SymphoniaSource {
    pub fn open(path: &Path) -> Result<SymphoniaSource> {
        SymphoniaSource::open_with(path, |reader| reader)
    }

    /// `open` with a hook on the freshly probed reader, which lets tests substitute a faulty one.
    pub fn open_with(
        path: &Path,
        wrap: impl FnOnce(Box<dyn FormatReader>) -> Box<dyn FormatReader>,
    ) -> Result<SymphoniaSource> {
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
        // Neither symphonia's ALAC reader nor decoder reports the depth, but its config does.
        let container_depth = params
            .bits_per_sample
            .or(params.bits_per_coded_sample)
            .or_else(|| {
                (params.codec == CODEC_ID_ALAC)
                    .then(|| params.extra_data.as_deref().and_then(alac_bit_depth))
                    .flatten()
            });
        let codec = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())?;

        // The decoder knows the final parameters (e.g. mp3 learns them from the first frame).
        let params = codec.codec_params();
        let sample_rate = params
            .sample_rate
            .ok_or(Error::Unsupported("unknown sample rate"))?;
        let channel_set = params
            .channels
            .as_ref()
            .ok_or(Error::Unsupported("unknown channel layout"))?;
        let channels = u16::try_from(channel_set.count())
            .ok()
            .filter(|&count| count > 0)
            .ok_or(Error::Unsupported("unknown channel layout"))?;
        let speakers = match channel_set {
            Channels::Positioned(position) => position.bits(),
            _ => 0,
        };
        let bit_depth = params
            .bits_per_sample
            .or(params.bits_per_coded_sample)
            .or(container_depth);
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

        Ok(SymphoniaSource {
            track_id: track.id,
            path: path.to_path_buf(),
            format: wrap(format),
            codec,
            time_base,
            info: Info {
                sample_rate,
                channels,
                layout: ChannelLayout::from_mask(speakers, channels),
                bit_depth,
                duration_ms,
            },
            samples: Vec::new(),
            origin_ts,
            end_ts,
            skip_until: None,
            pristine: true,
            decoded_any: false,
            seeked: false,
        })
    }

    fn read_packet(&mut self) -> Result<Option<&[f32]>> {
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
            let channels = usize::from(self.info.channels);
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

    fn seek_position(&mut self, position_ms: u64) -> Result<u64> {
        // symphonia's readers can lose their place when seeking after reads (mp4 once it has hit
        // the end, flac inside its buffer), so only a freshly opened reader is trusted to seek.
        if !self.pristine {
            *self = SymphoniaSource::open_with(&self.path, |reader| reader)?;
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
        let frames = ticks * u128::from(numer.get()) * u128::from(self.info.sample_rate)
            / u128::from(denom.get());
        usize::try_from(frames).unwrap_or(usize::MAX)
    }
}

impl Source for SymphoniaSource {
    fn backend(&self) -> Backend {
        Backend::Symphonia
    }

    fn info(&self) -> Info {
        self.info
    }

    fn read_chunk(&mut self) -> Result<Option<&[f32]>> {
        self.read_packet()
    }

    fn seek_to(&mut self, position_ms: u64) -> Result<u64> {
        self.seek_position(position_ms)
    }
}

/// The bit depth in an ALAC "magic cookie": 24 bytes, possibly preceded by `frma` and `alac`
/// atoms, with the depth as the sixth byte.
fn alac_bit_depth(mut cookie: &[u8]) -> Option<u32> {
    for atom in [b"frma", b"alac"] {
        if cookie.get(4..8) == Some(atom) {
            cookie = cookie.get(12..)?;
        }
    }
    cookie.get(5).map(|&depth| u32::from(depth))
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
    pub fn open_panics(_: &Path) -> Result<crate::decoder::Decoder> {
        let real = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sine.flac");
        crate::decoder::Decoder::open_with(&real, |_| panic!("injected fault while probing"))
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
