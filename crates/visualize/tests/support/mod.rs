//! Synthetic audio and a harness that plays it into an analyzer the way the engine would: in
//! blocks, each stamped with the time it will be heard, and a frame asked for as time goes by.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::time::Duration;

use antistreaming_visualize::{Analyzer, BANDS, Frame};

pub const RATE: u32 = 44_100;

pub fn secs(seconds: f64) -> Duration {
    Duration::from_secs_f64(seconds)
}

pub fn sine(hz: f32, amplitude: f32, seconds: f32, rate: u32) -> Vec<f32> {
    (0..(seconds * rate as f32) as usize)
        .map(|n| amplitude * (TAU * hz * n as f32 / rate as f32).sin())
        .collect()
}

pub fn silence(seconds: f32, rate: u32) -> Vec<f32> {
    vec![0.0; (seconds * rate as f32) as usize]
}

/// Deterministic noise in -1..1.
pub fn noise(seconds: f32, rate: u32, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    (0..(seconds * rate as f32) as usize)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        })
        .collect()
}

/// Both channels the same.
pub fn stereo(mono: &[f32]) -> Vec<f32> {
    mono.iter().flat_map(|&sample| [sample, sample]).collect()
}

pub fn interleave(left: &[f32], right: &[f32]) -> Vec<f32> {
    left.iter().zip(right).flat_map(|(&l, &r)| [l, r]).collect()
}

/// `signal` with `burst` added at each of `times` (seconds).
pub fn with_bursts(length: f32, rate: u32, times: &[f32], burst: &[f32]) -> Vec<f32> {
    let mut signal = silence(length, rate);
    for &time in times {
        let start = (time * rate as f32) as usize;
        for (n, &sample) in burst.iter().enumerate() {
            if let Some(slot) = signal.get_mut(start + n) {
                *slot += sample;
            }
        }
    }
    signal
}

/// A kick drum: a decaying sine.
pub fn kick(rate: u32) -> Vec<f32> {
    (0..(0.12 * rate as f32) as usize)
        .map(|n| {
            let t = n as f32 / rate as f32;
            0.9 * (TAU * 55.0 * t).sin() * (-t * 30.0).exp()
        })
        .collect()
}

/// A hard click: a few samples of noise.
pub fn click(rate: u32) -> Vec<f32> {
    let _ = rate;
    let mut burst = noise(0.002, rate, 7);
    for (n, sample) in burst.iter_mut().enumerate() {
        *sample *= 0.9 * (-(n as f32) / 20.0).exp();
    }
    burst
}

pub fn beat_times(bpm: f32, from: f32, until: f32) -> Vec<f32> {
    let period = 60.0 / bpm;
    (0..)
        .map(|n| from + n as f32 * period)
        .take_while(|&t| t < until)
        .collect()
}

/// The frame at `t` seconds.
pub fn at(analyzer: &Analyzer, t: f64) -> Frame {
    analyzer
        .frame_at(secs(t))
        .unwrap_or_else(|| panic!("no frame at {t} s"))
}

pub fn loudest_band(frame: &Frame) -> usize {
    (0..BANDS)
        .max_by(|&a, &b| frame.bands[a].total_cmp(&frame.bands[b]))
        .unwrap()
}

/// The centre frequency of band `band` at `rate`: the bands span 30 Hz to 16 kHz, or to the
/// Nyquist frequency if that is lower.
pub fn band_hz(band: usize, rate: u32) -> f32 {
    let top = 16_000.0_f32.min(rate as f32 / 2.0);
    30.0 * (top / 30.0).powf((band as f32 + 0.5) / BANDS as f32)
}

/// What one run of audio through an analyzer produced.
pub struct Run {
    pub analyzer: Analyzer,
    /// Every frame seen as time went by, by its `t_ms`.
    pub frames: BTreeMap<u64, Frame>,
}

/// Pushes `interleaved` in blocks of `block` sample-frames starting at `start` seconds, asking for
/// a frame `lag` seconds behind the end of each block as a renderer polling at the display rate
/// would, and keeps what it saw.
pub fn play(
    interleaved: &[f32],
    channels: u16,
    rate: u32,
    block: usize,
    start: f64,
    lag: f64,
) -> Run {
    let mut analyzer = Analyzer::new(rate, channels);
    let mut frames = BTreeMap::new();
    let mut pushed = 0usize;
    for chunk in interleaved.chunks(block * usize::from(channels)) {
        let at = start + pushed as f64 / f64::from(rate);
        analyzer.push(chunk, secs(at));
        pushed += chunk.len() / usize::from(channels);
        let now = start + pushed as f64 / f64::from(rate);
        if let Some(frame) = analyzer.frame_at(secs((now - lag).max(0.0))) {
            frames.insert(frame.t_ms, frame);
        }
    }
    Run { analyzer, frames }
}

impl Run {
    /// The frame seen nearest `t` seconds.
    pub fn near(&self, t: f64) -> &Frame {
        let wanted = (t * 1000.0) as i64;
        self.frames
            .values()
            .min_by_key(|frame| (frame.t_ms as i64 - wanted).abs())
            .expect("frames")
    }
}
