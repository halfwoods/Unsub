use serde::{Serialize, Serializer};

/// Number of log-spaced spectrum bands in a [`Frame`].
pub const BANDS: usize = 64;
/// Number of points in each waveform of a [`Frame`].
pub const SCOPE_POINTS: usize = 256;

/// What the visualizer draws for one moment of the music. Serialized with camelCase names, which
/// the renderer reads as they are.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// The presentation time of the frame, in milliseconds on the clock the audio was pushed with.
    /// It names the frame, so a beat can be told apart from the same frame seen twice.
    pub t_ms: u64,
    /// Loudness of each band from 30 Hz to 16 kHz, spaced evenly in log frequency: 0 is -70 dB
    /// (or silence) and 1 is 0 dB, after a gentle A-weighting and attack/release smoothing.
    #[serde(serialize_with = "sequence")]
    pub bands: [f32; BANDS],
    /// A cap for each band that sits at its highest recent value, then falls back.
    #[serde(serialize_with = "sequence")]
    pub peaks: [f32; BANDS],
    /// The left channel, -1 to 1, started at a rising zero crossing so a steady tone holds still.
    #[serde(serialize_with = "sequence")]
    pub waveform_l: [f32; SCOPE_POINTS],
    /// The right channel, on the same time axis as `waveform_l` (a mono source gives the same).
    #[serde(serialize_with = "sequence")]
    pub waveform_r: [f32; SCOPE_POINTS],
    /// Root-mean-square level of both channels, 0 to 1 on the linear amplitude scale (a full-scale
    /// sine is 0.707).
    pub rms: f32,
    /// True on the one frame a beat lands on.
    pub beat: bool,
    /// How far the beat stood out from the music around it, 0 to 1; 0 when `beat` is false.
    pub beat_strength: f32,
    /// How suddenly the spectrum changed, 0 to 1 (spectral flux).
    pub onset: f32,
    /// The tempo in beats per minute once the rhythm has held steady for a few seconds.
    pub tempo_bpm: Option<f32>,
}

impl Frame {
    pub(crate) fn silent() -> Self {
        Self {
            t_ms: 0,
            bands: [0.0; BANDS],
            peaks: [0.0; BANDS],
            waveform_l: [0.0; SCOPE_POINTS],
            waveform_r: [0.0; SCOPE_POINTS],
            rms: 0.0,
            beat: false,
            beat_strength: 0.0,
            onset: 0.0,
            tempo_bpm: None,
        }
    }
}

/// serde implements `Serialize` for arrays only up to 32 elements.
fn sequence<S: Serializer, const N: usize>(
    values: &[f32; N],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(values)
}
