use std::time::Duration;

use crate::Result;

/// What is inside an audio file, as far as a speaker's ability to play it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Mp3,
    Aac,
    Alac,
    Flac,
    Vorbis,
    Opus,
    Wav,
    Aiff,
    /// WavPack, Monkey's Audio, DSD and the like: only the player's own decoders read them.
    Other,
}

impl Codec {
    /// The library's name for a codec.
    pub fn named(codec: &str) -> Self {
        match codec {
            "MP3" => Self::Mp3,
            "AAC" => Self::Aac,
            "ALAC" => Self::Alac,
            "FLAC" => Self::Flac,
            "Vorbis" => Self::Vorbis,
            "Opus" => Self::Opus,
            "WAV" => Self::Wav,
            "AIFF" => Self::Aiff,
            _ => Self::Other,
        }
    }

    /// The HTTP content type of a file of this codec; only meaningful for the codecs a speaker plays.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Aac | Self::Alac => "audio/mp4",
            Self::Flac => "audio/flac",
            Self::Vorbis | Self::Opus => "audio/ogg",
            Self::Wav => "audio/wav",
            Self::Aiff => "audio/aiff",
            Self::Other => "application/octet-stream",
        }
    }

    /// The file extension speakers expect at the end of a link: some decide how to decode by it.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Aac | Self::Alac => "m4a",
            Self::Flac => "flac",
            Self::Vorbis => "ogg",
            Self::Opus => "opus",
            Self::Wav => "wav",
            Self::Aiff => "aiff",
            Self::Other => "bin",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub codec: Codec,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
}

/// A song as a speaker is told about it: where to fetch it, and what to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Media {
    pub url: String,
    pub mime: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: Option<String>,
    /// Zero when unknown.
    pub duration_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Nothing is playing: never started, stopped, or played out.
    Stopped,
    /// Loaded, and waiting for enough audio to start.
    Buffering,
    Playing,
    Paused,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub state: State,
    pub position_ms: u64,
    /// Zero when the speaker does not say.
    pub duration_ms: u64,
    /// 0.0 to 1.0, when the speaker reported it.
    pub volume: Option<f32>,
    /// What the speaker has loaded, so that a preloaded song taking over is noticed.
    pub url: Option<String>,
    /// The speaker said the song played to its end, as opposed to being stopped. Speakers that do
    /// not say leave it false, and the end is told by the position.
    pub finished: bool,
    /// The speaker could not play what it was given.
    pub error: Option<String>,
}

impl Status {
    /// What the speaker would report `elapsed` later, if nothing changed in between. Speakers are
    /// asked about once a second or so; this fills in the time between.
    pub fn advanced(&self, elapsed: Duration) -> Status {
        let mut status = self.clone();
        if status.state == State::Playing {
            status.position_ms += elapsed.as_millis() as u64;
            if status.duration_ms > 0 {
                status.position_ms = status.position_ms.min(status.duration_ms);
            }
        }
        status
    }
}

/// One speaker, once it is connected. Calls block until the speaker answers; they come from one
/// thread at a time.
pub trait Receiver: Send {
    /// Replaces whatever is playing with `media`, starting `start_ms` into it, and playing it unless
    /// `autoplay` is false.
    fn load(&mut self, media: &Media, start_ms: u64, autoplay: bool) -> Result<()>;

    /// Has the speaker play `media` straight after the current song. `Ok(false)` means it cannot,
    /// and the song has to be loaded when the current one ends.
    fn preload(&mut self, media: &Media) -> Result<bool> {
        let _ = media;
        Ok(false)
    }

    /// Takes back what [`Receiver::preload`] offered.
    fn clear_preload(&mut self) -> Result<()> {
        Ok(())
    }

    fn play(&mut self) -> Result<()>;
    fn pause(&mut self) -> Result<()>;
    fn seek(&mut self, position_ms: u64) -> Result<()>;

    /// `volume` is 0.0 to 1.0.
    fn set_volume(&mut self, volume: f32) -> Result<()>;

    fn status(&mut self) -> Result<Status>;

    /// Stops playing, leaving the speaker connected.
    fn stop(&mut self) -> Result<()>;

    /// Lets go of the speaker: whatever app was started on it is closed.
    fn disconnect(&mut self);
}
