//! Loudness measurement for Sound Check: EBU R128 / ITU-R BS.1770-4 integrated loudness and true
//! peak, plus the per-block data that lets an album's loudness be worked out later without
//! decoding anything again.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use ebur128::{Channel, EbuR128, Mode};
use serde::{Deserialize, Serialize};

use crate::decoder::Decoder;
use crate::mix::ChannelLayout;
use crate::{Error, Result};

/// BS.1770-4 drops gating blocks quieter than this before anything else.
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// Then drops blocks this far below the average of those that remain.
const RELATIVE_GATE_LU: f64 = -10.0;
/// Resolution of [`BlockHistogram`]. Blocks are placed to within half a bin, which moves an
/// album's integrated loudness by a few thousandths of a LU in practice (0.025 LU at worst),
/// well inside EBU Tech 3341's +-0.1 LU.
const BINS_PER_LU: f64 = 20.0;
/// Progress is reported once per second of audio, not once per decoded packet.
const PROGRESS_STEP_MS: u64 = 1000;

/// The loudness of a track, or of an album once combined by [`album_loudness`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Loudness {
    /// Gated programme loudness, `-inf` when nothing is above the absolute gate: silence, or
    /// audio shorter than one 400 ms block.
    #[serde(with = "silence_as_null")]
    pub integrated_lufs: f64,
    /// Highest inter-sample peak of any channel, from 4x oversampling. `-inf` for digital silence.
    #[serde(with = "silence_as_null")]
    pub true_peak_dbtp: f64,
    /// Length of the audio that was measured.
    pub duration_ms: u64,
    /// What [`album_loudness`] needs from this track. Store it with the numbers above.
    pub blocks: BlockHistogram,
}

/// How many 400 ms gating blocks of a track fall at each loudness.
///
/// Gated loudness is not an average of per-track results: whether a block counts depends on the
/// mean of every block in the programme, so a quiet track can be gated out of an album that a
/// loud one dominates. Combining the blocks themselves gives the loudness of the album as one
/// continuous programme. This is what libebur128 and FFmpeg keep too; the sparse form costs a
/// few kilobytes per track.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BlockHistogram(
    /// `(bin, block count)` in ascending bin order. Bin 0 starts at the absolute gate.
    Vec<(u16, u32)>,
);

impl BlockHistogram {
    fn add(&mut self, lufs: f64) {
        if lufs < ABSOLUTE_GATE_LUFS {
            return;
        }
        // Float to integer casts saturate, so a nonsense level lands in the last bin.
        let bin = ((lufs - ABSOLUTE_GATE_LUFS) * BINS_PER_LU) as u16;
        self.add_count(bin, 1);
    }

    fn add_count(&mut self, bin: u16, count: u32) {
        match self.0.binary_search_by_key(&bin, |&(bin, _)| bin) {
            Ok(at) => self.0[at].1 += count,
            Err(at) => self.0.insert(at, (bin, count)),
        }
    }

    fn merge(&mut self, other: &BlockHistogram) {
        for &(bin, count) in &other.0 {
            self.add_count(bin, count);
        }
    }

    /// BS.1770-4 gated loudness of the blocks, taking each at the centre of its bin.
    fn integrated_lufs(&self) -> f64 {
        let power = |bin: u16| {
            10f64.powf((ABSOLUTE_GATE_LUFS + (f64::from(bin) + 0.5) / BINS_PER_LU) / 10.0)
        };
        let mean_power = |threshold: f64| {
            let (blocks, total) = self
                .0
                .iter()
                .filter(|&&(bin, _)| power(bin) >= threshold)
                .fold((0u64, 0.0), |(blocks, total), &(bin, count)| {
                    (
                        blocks + u64::from(count),
                        total + f64::from(count) * power(bin),
                    )
                });
            (blocks > 0).then(|| total / blocks as f64)
        };
        // The mean of power is the mean of energy up to the constant that turns it into LUFS,
        // which the relative gate and the result both carry through unchanged.
        let Some(ungated) = mean_power(0.0) else {
            return f64::NEG_INFINITY;
        };
        let relative = ungated * 10f64.powf(RELATIVE_GATE_LU / 10.0);
        mean_power(relative).map_or(f64::NEG_INFINITY, |gated| 10.0 * gated.log10())
    }
}

/// Loudness of `tracks` played as one programme, as Sound Check's album gain needs. The result
/// is itself a [`Loudness`], so it combines again (discs into a box set). Empty input is silence.
///
/// Its `integrated_lufs` comes from the histograms, so it can differ from a single track's own
/// figure by a few thousandths of a LU; the peak is the highest of the tracks'.
pub fn album_loudness(tracks: &[Loudness]) -> Loudness {
    let mut blocks = BlockHistogram::default();
    for track in tracks {
        blocks.merge(&track.blocks);
    }
    Loudness {
        integrated_lufs: blocks.integrated_lufs(),
        true_peak_dbtp: tracks
            .iter()
            .map(|track| track.true_peak_dbtp)
            .fold(f64::NEG_INFINITY, f64::max),
        duration_ms: tracks.iter().map(|track| track.duration_ms).sum(),
        blocks,
    }
}

/// Measures the loudness of the file at `path`, decoding it exactly as playback would so DSD,
/// Opus and the rest need nothing special. `progress` gets `(measured_ms, total_ms)` about once
/// per second of audio (`total_ms` is 0 if the file does not say how long it is) and `cancel`
/// stops the measurement with [`Error::Cancelled`].
pub fn analyze_loudness(
    path: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<Loudness> {
    let mut decoder = Decoder::open(path)?;
    let total_ms = decoder.duration_ms().unwrap_or(0);
    let mut meter = Meter::new(decoder.channel_layout(), decoder.sample_rate())?;
    let mut reported_ms = 0;
    while let Some(chunk) = decoder.next_chunk()? {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        meter.push(chunk)?;
        if meter.elapsed_ms() >= reported_ms + PROGRESS_STEP_MS {
            reported_ms = meter.elapsed_ms();
            progress(reported_ms, total_ms);
        }
    }
    let measured_ms = meter.elapsed_ms();
    progress(measured_ms, measured_ms);
    meter.finish()
}

/// An `ebur128` analyzer that also records every gating block, which the crate keeps to itself.
struct Meter {
    analyzer: EbuR128,
    blocks: BlockHistogram,
    channels: usize,
    rate: u64,
    /// Frames in 100 ms, the distance between gating blocks. Rounded as the analyzer does, so
    /// that the two agree on where a block ends.
    step: usize,
    /// Frames until the analyzer completes its next block: the first needs 400 ms of audio,
    /// every later one 100 ms more.
    until_block: usize,
    frames: u64,
}

impl Meter {
    fn new(layout: ChannelLayout, rate: u32) -> Result<Self> {
        let channels = layout.channels();
        let mut analyzer = EbuR128::new(u32::from(channels), rate, Mode::I | Mode::TRUE_PEAK)?;
        if let Some(map) = channel_map(layout) {
            analyzer.set_channel_map(&map)?;
        }
        let step = (rate as usize + 5) / 10;
        Ok(Meter {
            analyzer,
            blocks: BlockHistogram::default(),
            channels: usize::from(channels),
            rate: u64::from(rate),
            step,
            until_block: 4 * step,
            frames: 0,
        })
    }

    fn elapsed_ms(&self) -> u64 {
        self.frames * 1000 / self.rate
    }

    /// Adds whole interleaved frames.
    fn push(&mut self, samples: &[f32]) -> Result<()> {
        let mut rest = samples;
        while !rest.is_empty() {
            // Stop at each block boundary to read the block the analyzer has just closed.
            let frames = self.until_block.min(rest.len() / self.channels);
            let (now, later) = rest.split_at(frames * self.channels);
            self.analyzer.add_frames_f32(now)?;
            self.frames += frames as u64;
            self.until_block -= frames;
            if self.until_block == 0 {
                self.blocks.add(self.analyzer.loudness_momentary()?);
                self.until_block = self.step;
            }
            rest = later;
        }
        Ok(())
    }

    fn finish(self) -> Result<Loudness> {
        let mut peak = 0f64;
        for channel in 0..self.channels as u32 {
            peak = peak.max(self.analyzer.true_peak(channel)?);
        }
        Ok(Loudness {
            integrated_lufs: self.analyzer.loudness_global()?,
            true_peak_dbtp: 20.0 * peak.log10(),
            duration_ms: self.elapsed_ms(),
            blocks: self.blocks,
        })
    }
}

/// BS.1770-4 channel roles for the layouts decoders emit, in their channel order. LFE is
/// `Unused` because the standard leaves it out of the measurement. A mono file counts twice,
/// as it sounds when a player sends it to both speakers. `None` leaves `ebur128`'s own map for
/// the channel count.
fn channel_map(layout: ChannelLayout) -> Option<Vec<Channel>> {
    use Channel::*;
    match layout {
        ChannelLayout::Mono => Some(vec![DualMono]),
        ChannelLayout::Stereo => Some(vec![Left, Right]),
        ChannelLayout::Surround5_1 => Some(vec![
            Left,
            Right,
            Center,
            Unused,
            LeftSurround,
            RightSurround,
        ]),
        // Rear pair at 135 degrees, side pair at 90: only the side pair gets the surround boost.
        ChannelLayout::Surround7_1 => Some(vec![
            Left, Right, Center, Unused, Mp135, Mm135, Mp090, Mm090,
        ]),
        ChannelLayout::Other(_) => None,
    }
}

/// JSON has no infinity and silence measures as `-inf`, so it is stored as `null`.
mod silence_as_null {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        if value.is_finite() {
            serializer.serialize_f64(*value)
        } else {
            serializer.serialize_none()
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(deserializer)?.unwrap_or(f64::NEG_INFINITY))
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    /// A deterministic stand-in for programme material: noise whose level changes every 100 ms.
    fn programme(rate: u32, channels: usize, levels_db: &[f32], seed: u32) -> Vec<f32> {
        let mut state = seed;
        let segment = (rate as usize + 5) / 10;
        let mut samples = Vec::new();
        for &level in levels_db {
            let amplitude = 10f32.powf(level / 20.0);
            for _ in 0..segment * channels {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                samples.push(amplitude * ((state >> 8) as f32 / (1 << 23) as f32 - 1.0));
            }
        }
        samples
    }

    fn measure(layout: ChannelLayout, rate: u32, samples: &[f32]) -> Meter {
        let mut meter = Meter::new(layout, rate).unwrap();
        // Odd-sized pushes: block ends must not depend on how the decoder cuts its packets.
        for chunk in samples.chunks(layout.channels() as usize * 997) {
            meter.push(chunk).unwrap();
        }
        meter
    }

    fn levels(seed: u32, count: usize, base_db: f32) -> Vec<f32> {
        (0..count)
            .map(|n| base_db - ((n as u32 * 7 + seed) % 23) as f32)
            .collect()
    }

    #[test]
    fn a_tracks_blocks_compose_to_the_analyzers_own_figure() {
        for rate in [11_025, 22_050, 44_100, 48_000] {
            let samples = programme(rate, 2, &levels(3, 120, -12.0), 1);
            let meter = measure(ChannelLayout::Stereo, rate, &samples);
            let exact = meter.analyzer.loudness_global().unwrap();
            let composed = album_loudness(&[meter.finish().unwrap()]).integrated_lufs;
            assert!(
                (exact - composed).abs() < 0.01,
                "{rate} Hz: {exact} vs {composed}"
            );
        }
    }

    #[test]
    fn an_album_matches_the_reference_implementation_over_live_analyzers() {
        let rate = 44_100;
        let meters: Vec<Meter> = [(-8.0, 1), (-20.0, 2), (-35.0, 3)]
            .iter()
            .map(|&(base, seed)| {
                let samples = programme(rate, 2, &levels(seed, 150, base), seed);
                measure(ChannelLayout::Stereo, rate, &samples)
            })
            .collect();
        let exact =
            EbuR128::loudness_global_multiple(meters.iter().map(|meter| &meter.analyzer)).unwrap();
        let tracks: Vec<Loudness> = meters.into_iter().map(|m| m.finish().unwrap()).collect();
        let album = album_loudness(&tracks).integrated_lufs;
        assert!((exact - album).abs() < 0.01, "{exact} vs {album}");
    }

    #[test]
    fn every_closed_block_is_recorded_once() {
        let rate = 22_050;
        let samples = programme(rate, 2, &levels(5, 100, -10.0), 9);
        let loudness = measure(ChannelLayout::Stereo, rate, &samples)
            .finish()
            .unwrap();
        // 100 segments of 100 ms make 97 blocks of 400 ms at a 100 ms hop.
        let recorded: u32 = loudness.blocks.0.iter().map(|&(_, count)| count).sum();
        assert_eq!(recorded, 97);
    }

    #[test]
    fn combining_is_associative_and_order_free() {
        let rate = 44_100;
        let tracks: Vec<Loudness> = (1..=3)
            .map(|seed| {
                let samples = programme(rate, 2, &levels(seed, 120, -9.0 * seed as f32), seed);
                measure(ChannelLayout::Stereo, rate, &samples)
                    .finish()
                    .unwrap()
            })
            .collect();
        let flat = album_loudness(&tracks);
        let nested = album_loudness(&[album_loudness(&tracks[..2]), tracks[2].clone()]);
        let reversed = album_loudness(&[tracks[2].clone(), tracks[0].clone(), tracks[1].clone()]);
        assert_eq!(flat.blocks, nested.blocks);
        assert_eq!(flat.blocks, reversed.blocks);
        assert_eq!(flat.integrated_lufs, nested.integrated_lufs);
        assert_eq!(flat.duration_ms, nested.duration_ms);
    }

    #[test]
    fn an_empty_album_is_silence() {
        let album = album_loudness(&[]);
        assert_eq!(album.integrated_lufs, f64::NEG_INFINITY);
        assert_eq!(album.true_peak_dbtp, f64::NEG_INFINITY);
        assert_eq!(album.duration_ms, 0);
    }

    #[test]
    fn a_tone_measures_the_same_however_it_is_laid_out_in_channels() {
        let rate = 48_000;
        let tone: Vec<f32> = (0..rate as usize * 3)
            .map(|n| 0.0708 * (TAU * 997.0 * n as f32 / rate as f32).sin())
            .collect();
        let stereo: Vec<f32> = tone.iter().flat_map(|&s| [s, s]).collect();
        let mono = measure(ChannelLayout::Mono, rate, &tone).finish().unwrap();
        let both = measure(ChannelLayout::Stereo, rate, &stereo)
            .finish()
            .unwrap();
        assert!((mono.integrated_lufs - both.integrated_lufs).abs() < 1e-6);
        assert!(
            (both.integrated_lufs + 23.0).abs() < 0.05,
            "{}",
            both.integrated_lufs
        );
    }

    #[test]
    fn json_keeps_silence_and_the_blocks() {
        let silent = measure(ChannelLayout::Stereo, 44_100, &vec![0.0; 44_100 * 2])
            .finish()
            .unwrap();
        assert_eq!(silent.integrated_lufs, f64::NEG_INFINITY);
        let json = serde_json::to_string(&silent).unwrap();
        assert!(json.contains("\"integratedLufs\":null"), "{json}");
        assert_eq!(serde_json::from_str::<Loudness>(&json).unwrap(), silent);

        let samples = programme(44_100, 2, &levels(4, 80, -14.0), 4);
        let loud = measure(ChannelLayout::Stereo, 44_100, &samples)
            .finish()
            .unwrap();
        let json = serde_json::to_string(&loud).unwrap();
        assert_eq!(serde_json::from_str::<Loudness>(&json).unwrap(), loud);
    }
}
