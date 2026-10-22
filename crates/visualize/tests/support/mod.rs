//! Synthetic audio and a harness that plays it into an analyzer the way the engine would: in
//! blocks, each stamped with the time it will be heard, and a frame asked for as time goes by.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::time::Duration;

use unsub_visualize::{Analyzer, BANDS, Frame};

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

/// Adds `sound`, scaled by `level`, into `signal` from `at` seconds.
pub fn add(signal: &mut [f32], rate: u32, at: f32, sound: &[f32], level: f32) {
    let start = (at * rate as f32) as usize;
    for (slot, &sample) in signal.iter_mut().skip(start).zip(sound) {
        *slot += level * sample;
    }
}

/// A kick as a drum machine makes one: a sine falling from 150 Hz to 48 Hz within 40 ms, with a
/// body that lasts a quarter of a second and a beater's click on top. Peak about 1.
pub fn drum_kick(rate: u32) -> Vec<f32> {
    let hiss = noise(0.3, rate, 19);
    let mut phase = 0.0_f32;
    (0..(0.3 * rate as f32) as usize)
        .map(|n| {
            let t = n as f32 / rate as f32;
            let hz = 48.0 + 102.0 * (-t * 70.0).exp();
            phase += TAU * hz / rate as f32;
            let body = phase.sin() * (1.0 - (-t * 1500.0).exp()) * (-t * 11.0).exp();
            let click = (hiss[n] - hiss[n.saturating_sub(1)]) * 0.35 * (-t * 400.0).exp();
            body + click
        })
        .collect()
}

/// A snare: a burst of noise and a 190 Hz body, 0.18 s. Peak about 1.
pub fn snare(rate: u32) -> Vec<f32> {
    let hiss = noise(0.18, rate, 31);
    hiss.iter()
        .enumerate()
        .map(|(n, &hiss)| {
            let t = n as f32 / rate as f32;
            (0.7 * hiss * (-t * 22.0).exp() + 0.5 * (TAU * 190.0 * t).sin() * (-t * 30.0).exp())
                * (1.0 - (-t * 2000.0).exp())
        })
        .collect()
}

/// A closed hi-hat: noise with its lows taken away, 50 ms. Peak about 0.5.
pub fn hat(rate: u32) -> Vec<f32> {
    let hiss = noise(0.05, rate, 53);
    let mut previous = 0.0;
    hiss.iter()
        .enumerate()
        .map(|(n, &hiss)| {
            let bright = hiss - previous;
            previous = hiss;
            0.5 * bright * (-(n as f32) / rate as f32 * 80.0).exp()
        })
        .collect()
}

/// A pad: a chord of sines with their octaves and a little hiss, swelling slowly, changing
/// chord every `change` seconds with a one-second crossfade. Roughly steady in level.
pub fn pad(seconds: f32, rate: u32, change: f32) -> Vec<f32> {
    const CHORDS: [[f32; 3]; 4] = [
        [110.0, 130.8, 164.8],
        [87.3, 130.8, 174.6],
        [98.0, 123.5, 146.8],
        [110.0, 138.6, 164.8],
    ];
    let hiss = noise(seconds, rate, 77);
    (0..(seconds * rate as f32) as usize)
        .map(|n| {
            let t = n as f32 / rate as f32;
            let index = (t / change) as usize;
            let into = (t - index as f32 * change).min(1.0);
            let note = |chord: &[f32; 3]| -> f32 {
                chord
                    .iter()
                    .map(|&hz| {
                        (TAU * hz * t).sin()
                            + 0.5 * (TAU * 2.0 * hz * t).sin()
                            + 0.25 * (TAU * 4.0 * hz * t).sin()
                    })
                    .sum::<f32>()
                    / 6.0
            };
            let now = note(&CHORDS[index % 4]);
            let blended = if index > 0 && into < 1.0 {
                note(&CHORDS[(index - 1) % 4]) * (1.0 - into) + now * into
            } else {
                now
            };
            (blended * (0.85 + 0.15 * (TAU * 0.27 * t).sin()) + 0.02 * hiss[n]) * 0.6
        })
        .collect()
}

/// A bass line: one sustained sine per step of `step` seconds, with a short attack and release.
pub fn bass_line(seconds: f32, rate: u32, notes: &[f32], step: f32) -> Vec<f32> {
    (0..(seconds * rate as f32) as usize)
        .map(|n| {
            let t = n as f32 / rate as f32;
            let index = (t / step) as usize;
            let into = t - index as f32 * step;
            let envelope = (into / 0.01).min(1.0) * ((step - into) / 0.03).clamp(0.0, 1.0);
            envelope * (TAU * notes[index % notes.len()] * t).sin()
        })
        .collect()
}

/// A plucked note: a sine and its octave, 5 ms attack, `decay` seconds to fall to a third.
pub fn pluck(hz: f32, decay: f32, rate: u32) -> Vec<f32> {
    (0..(decay * 5.0 * rate as f32) as usize)
        .map(|n| {
            let t = n as f32 / rate as f32;
            ((TAU * hz * t).sin() + 0.3 * (TAU * 2.0 * hz * t).sin())
                * (t / 0.005).min(1.0)
                * (-t * 1.1 / decay).exp()
        })
        .collect()
}

/// What a mastering limiter does to a mix: loud parts are pushed into the ceiling, so sustained
/// sound crowds the hits that stood out of it.
pub fn master(signal: &mut [f32], drive: f32) {
    let norm = drive.tanh();
    for sample in signal {
        *sample = (drive * *sample).tanh() / norm;
    }
}

/// How well `beats` (seconds) match the real `hits` (seconds): the share of hits after `from`
/// that have a beat within `tolerance`, and the beats after `from` that are near no hit.
pub fn score(beats: &[f64], hits: &[f32], from: f32, tolerance: f64) -> (f32, usize) {
    let counted: Vec<f64> = hits
        .iter()
        .filter(|&&hit| hit >= from)
        .map(|&hit| f64::from(hit))
        .collect();
    let caught = counted
        .iter()
        .filter(|&&hit| beats.iter().any(|&beat| (beat - hit).abs() <= tolerance))
        .count();
    let stray = beats
        .iter()
        .filter(|&&beat| {
            beat >= f64::from(from)
                && hits
                    .iter()
                    .all(|&hit| (beat - f64::from(hit)).abs() > tolerance)
        })
        .count();
    (caught as f32 / counted.len().max(1) as f32, stray)
}

/// The times of the beats a run found.
pub fn beat_seconds(run: &Run) -> Vec<f64> {
    run.frames
        .values()
        .filter(|frame| frame.beat)
        .map(|frame| frame.t_ms as f64 / 1000.0)
        .collect()
}
