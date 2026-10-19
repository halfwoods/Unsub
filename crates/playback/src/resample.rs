use rubato::audioadapter::Adapter;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as _};

use crate::Result;
use crate::mix::Mixer;

/// Input frames per FFT call; large enough to amortise setup, small enough to keep latency low.
const CHUNK_FRAMES: usize = 1024;

/// The flush after the last input needs at most one chunk to emit the filter delay; the extra
/// is a guard so a misbehaving resampler cannot spin the engine thread.
const MAX_FLUSH_CHUNKS: usize = 4;

/// Converts decoded audio to the device's sample rate and channel count.
pub struct Resampler {
    fft: Option<Fft<f32>>,
    mixer: Mixer,
    to_channels: usize,
    ratio: f64,
    /// Channel-mapped input waiting for a full FFT chunk.
    pending: Vec<f32>,
    scratch: Vec<f32>,
    /// Startup frames the FFT emits before the signal begins.
    skip_frames: usize,
    frames_in: u64,
    frames_out: u64,
}

impl Resampler {
    /// Converts from `from_rate` to `to_rate`, mixing channels with `mixer` first.
    pub fn new(from_rate: u32, to_rate: u32, mixer: Mixer) -> Result<Self> {
        let to_channels = mixer.device_channels();
        let fft = if from_rate == to_rate {
            None
        } else {
            Some(Fft::new(
                from_rate as usize,
                to_rate as usize,
                CHUNK_FRAMES,
                to_channels,
                FixedSync::Input,
            )?)
        };
        let scratch_frames = fft.as_ref().map_or(0, |fft| fft.output_frames_max());
        let skip_frames = fft.as_ref().map_or(0, |fft| fft.output_delay());

        Ok(Resampler {
            fft,
            mixer,
            to_channels,
            ratio: f64::from(to_rate) / f64::from(from_rate),
            pending: Vec::new(),
            scratch: vec![0.0; scratch_frames * to_channels],
            skip_frames,
            frames_in: 0,
            frames_out: 0,
        })
    }

    /// Appends the converted form of `input` (interleaved, source channel count) to `out`.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        let Some(fft) = self.fft.as_mut() else {
            self.mixer.mix(input, out);
            return Ok(());
        };

        self.frames_in += (input.len() / self.mixer.source_channels()) as u64;
        self.mixer.mix(input, &mut self.pending);

        let channels = self.to_channels;
        loop {
            let frames = fft.input_frames_next();
            if self.pending.len() < frames * channels {
                return Ok(());
            }
            let chunk =
                InterleavedSlice::new(&self.pending[..frames * channels], channels, frames)?;
            self.frames_out += run_chunk(
                fft,
                &chunk,
                None,
                &mut self.scratch,
                &mut self.skip_frames,
                out,
            )?;
            self.pending.drain(..frames * channels);
        }
    }

    /// Emits the audio still held in the filter, then readies the resampler for a new stream.
    pub fn finish(&mut self, out: &mut Vec<f32>) -> Result<()> {
        if let Some(fft) = self.fft.as_mut() {
            let expected = (self.frames_in as f64 * self.ratio).round() as u64;
            let channels = self.to_channels;
            let frames = fft.input_frames_next();
            let mut remaining = self.pending.len() / channels;

            for _ in 0..MAX_FLUSH_CHUNKS {
                if self.frames_out >= expected {
                    break;
                }
                // Padding the final chunk with silence pushes the buffered tail through.
                self.pending.resize(frames * channels, 0.0);
                let chunk = InterleavedSlice::new(&self.pending[..], channels, frames)?;
                self.frames_out += run_chunk(
                    fft,
                    &chunk,
                    Some(remaining),
                    &mut self.scratch,
                    &mut self.skip_frames,
                    out,
                )?;
                self.pending.clear();
                remaining = 0;
            }

            // Silence padding makes the last chunk overshoot the true length.
            let excess = usize::try_from(self.frames_out.saturating_sub(expected)).unwrap_or(0);
            out.truncate(out.len().saturating_sub(excess * channels));
        }
        self.reset();
        Ok(())
    }

    /// Drops buffered audio, for use after a seek.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.frames_in = 0;
        self.frames_out = 0;
        if let Some(fft) = self.fft.as_mut() {
            fft.reset();
            self.skip_frames = fft.output_delay();
        }
    }
}

/// Runs one FFT chunk, appends its audio (minus filter startup silence) to `out`, and returns the
/// number of frames appended.
fn run_chunk(
    fft: &mut Fft<f32>,
    input: &InterleavedSlice<&[f32]>,
    partial_len: Option<usize>,
    scratch: &mut [f32],
    skip_frames: &mut usize,
    out: &mut Vec<f32>,
) -> Result<u64> {
    let channels = input.channels();
    let capacity = scratch.len() / channels;
    let mut output = InterleavedSlice::new_mut(scratch, channels, capacity)?;
    let indexing = partial_len.map(|frames| Indexing::new().partial_len(frames));
    let (_, produced) = fft.process_into_buffer(input, &mut output, indexing.as_ref())?;

    let startup = (*skip_frames).min(produced);
    *skip_frames -= startup;
    out.extend_from_slice(&scratch[startup * channels..produced * channels]);
    Ok((produced - startup) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::ChannelLayout;

    fn mixer(from: u16, to: u16) -> Mixer {
        Mixer::new(ChannelLayout::from_mask(0, from), to)
    }

    fn sine(rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|n| (2.0 * std::f32::consts::PI * 440.0 * n as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn mono_is_duplicated_to_stereo() {
        let mut resampler = Resampler::new(48_000, 48_000, mixer(1, 2)).unwrap();
        let mut out = Vec::new();
        resampler.push(&[0.25, -0.5], &mut out).unwrap();
        assert_eq!(out, [0.25, 0.25, -0.5, -0.5]);
    }

    #[test]
    fn same_format_is_passed_through_untouched() {
        let mut resampler = Resampler::new(44_100, 44_100, mixer(2, 2)).unwrap();
        let input = [0.1, 0.2, 0.3, 0.4];
        let mut out = Vec::new();
        resampler.push(&input, &mut out).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn surround_is_downmixed_before_the_rate_changes() {
        let layout = ChannelLayout::Surround5_1;
        let mut resampler = Resampler::new(44_100, 48_000, Mixer::new(layout, 2)).unwrap();
        // Front-left only, a whole second: left stays left through the downmix and the filter.
        let frame = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let input: Vec<f32> = frame.iter().copied().cycle().take(6 * 44_100).collect();
        let mut out = Vec::new();
        resampler.push(&input, &mut out).unwrap();
        resampler.finish(&mut out).unwrap();

        assert_eq!(out.len() / 2, 48_000);
        let middle = &out[2 * 24_000..2 * 24_001];
        assert!(middle[0] > 0.3 && middle[1].abs() < 1e-3, "{middle:?}");
    }

    #[test]
    fn resampled_length_matches_the_rate_ratio_once_flushed() {
        let mut resampler = Resampler::new(44_100, 48_000, mixer(1, 2)).unwrap();
        let input = sine(44_100, 44_100);
        let mut out = Vec::new();
        for chunk in input.chunks(1152) {
            resampler.push(chunk, &mut out).unwrap();
        }
        resampler.finish(&mut out).unwrap();
        assert_eq!(out.len() / 2, 48_000);
    }

    #[test]
    fn resampled_signal_keeps_its_level_and_alignment() {
        let mut resampler = Resampler::new(44_100, 48_000, mixer(1, 1)).unwrap();
        let input = sine(44_100, 44_100);
        let mut out = Vec::new();
        for chunk in input.chunks(1000) {
            resampler.push(chunk, &mut out).unwrap();
        }
        resampler.finish(&mut out).unwrap();

        // Compare mid-stream against the ideal 48 kHz sine; the edges carry filter transients.
        let ideal = sine(48_000, 48_000);
        let worst = (4_800..43_200)
            .map(|n| (out[n] - ideal[n]).abs())
            .fold(0.0_f32, f32::max);
        assert!(worst < 0.01, "worst deviation {worst}");
    }

    #[test]
    fn reset_forgets_buffered_input() {
        let mut resampler = Resampler::new(44_100, 48_000, mixer(1, 1)).unwrap();
        let mut out = Vec::new();
        resampler.push(&sine(44_100, 500), &mut out).unwrap();
        resampler.reset();
        let mut flushed = Vec::new();
        resampler.finish(&mut flushed).unwrap();
        assert!(flushed.is_empty());
    }
}
