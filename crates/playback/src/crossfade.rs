//! Crossfading one track into the next. The end of the stream is held back instead of being
//! queued for the device, so that the start of the next track can be blended into it.

use std::collections::VecDeque;
use std::f64::consts::FRAC_PI_2;

/// Gains `(outgoing, incoming)` for frame `index` of a fade `len` frames long. Quarter-wave
/// sine and cosine: their squares add up to one, so unrelated music keeps the same power
/// throughout, where a straight line would dip by 3 dB in the middle. Each runs from the end
/// of a plain stretch at gain one to the start of another, with no step at either side.
pub fn curves(index: usize, len: usize) -> (f32, f32) {
    let angle = index as f64 / len as f64 * FRAC_PI_2;
    (angle.cos() as f32, angle.sin() as f32)
}

/// Frames decoded but not yet handed on, which are the last stretch of the stream so far.
pub struct Holdback {
    channels: usize,
    queue: VecDeque<f32>,
    mix: Option<Mix>,
}

/// A blend in progress over the front of the queue.
struct Mix {
    len: usize,
    /// Frames of the incoming track blended in so far.
    done: usize,
}

impl Holdback {
    pub fn new(channels: usize) -> Self {
        Holdback {
            channels,
            queue: VecDeque::new(),
            mix: None,
        }
    }

    pub fn frames(&self) -> usize {
        self.queue.len() / self.channels
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.mix = None;
    }

    /// Takes in decoded frames. While a blend is open the first of them are mixed into the
    /// held tail as they arrive; the rest are queued behind it.
    pub fn absorb(&mut self, frames: &[f32]) {
        let mut rest = frames;
        if let Some(mix) = self.mix.as_mut() {
            let count = (mix.len - mix.done).min(rest.len() / self.channels);
            for frame in 0..count {
                let (outgoing, incoming) = curves(mix.done + frame, mix.len);
                for channel in 0..self.channels {
                    let held = &mut self.queue[(mix.done + frame) * self.channels + channel];
                    *held = *held * outgoing + rest[frame * self.channels + channel] * incoming;
                }
            }
            mix.done += count;
            rest = &rest[count * self.channels..];
            if mix.done == mix.len {
                self.mix = None;
            }
        }
        self.queue.extend(rest);
    }

    /// Moves all but the last `keep` frames to `out` and returns how many that was. Nothing
    /// moves while a blend is open: it sits at the front of the queue.
    pub fn release(&mut self, keep: usize, out: &mut Vec<f32>) -> usize {
        if self.mix.is_some() {
            return 0;
        }
        let count = self.frames().saturating_sub(keep);
        out.extend(self.queue.drain(..count * self.channels));
        count
    }

    /// Moves everything but the last `len` frames to `out`, returns how many that was, and
    /// opens a blend of the next frames into those `len`.
    pub fn begin_mix(&mut self, len: usize, out: &mut Vec<f32>) -> usize {
        let released = self.release(len, out);
        self.mix = Some(Mix { len, done: 0 });
        released
    }

    /// The incoming track ended before the blend did: the rest of the held tail fades out
    /// on its own rather than coming back at full volume.
    pub fn finish_mix(&mut self) {
        let Some(mix) = self.mix.take() else {
            return;
        };
        for frame in mix.done..mix.len {
            let (outgoing, _) = curves(frame, mix.len);
            for channel in 0..self.channels {
                self.queue[frame * self.channels + channel] *= outgoing;
            }
        }
    }
}

/// What a crossfade of `overlap` frames should make of two stereo tracks, worked out directly
/// for tests to compare the engine's stream with.
#[cfg(test)]
pub fn crossfaded(first: &[f32], second: &[f32], overlap: usize) -> Vec<f32> {
    let split = first.len() - overlap * 2;
    let mut stream = first[..split].to_vec();
    for frame in 0..overlap {
        let (outgoing, incoming) = curves(frame, overlap);
        for channel in 0..2 {
            stream.push(
                first[split + frame * 2 + channel] * outgoing
                    + second[frame * 2 + channel] * incoming,
            );
        }
    }
    stream.extend_from_slice(&second[overlap * 2..]);
    stream
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curves_always_add_up_to_the_same_power() {
        for len in [1, 2, 441, 44_100] {
            for index in 0..len {
                let (outgoing, incoming) = curves(index, len);
                assert!(
                    (outgoing * outgoing + incoming * incoming - 1.0).abs() < 1e-6,
                    "{index}/{len}"
                );
            }
        }
    }

    #[test]
    fn the_curves_run_smoothly_from_one_track_to_the_other() {
        let len = 1_000;
        assert_eq!(curves(0, len), (1.0, 0.0));
        let (outgoing, incoming) = curves(len - 1, len);
        // One frame short of the plain stretch that follows, where the incoming gain is one.
        assert!(
            outgoing < 0.01 && incoming > 0.9999,
            "{outgoing} {incoming}"
        );
        assert!((0..len - 1).all(|n| curves(n + 1, len).0 < curves(n, len).0));
        assert!((0..len - 1).all(|n| curves(n + 1, len).1 > curves(n, len).1));
        let (outgoing, incoming) = curves(len / 2, len);
        assert!((outgoing - incoming).abs() < 1e-6, "equal in the middle");
    }

    #[test]
    fn frames_beyond_the_ones_to_keep_are_released_in_order() {
        let mut held = Holdback::new(2);
        held.absorb(&[1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0]);
        let mut out = Vec::new();
        assert_eq!(held.release(3, &mut out), 1);
        assert_eq!(out, [1.0, 1.0]);
        assert_eq!(held.release(3, &mut out), 0);
        assert_eq!(held.frames(), 3);
        assert_eq!(held.release(0, &mut out), 3);
        assert_eq!(out, [1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0]);
        assert!(held.is_empty());
    }

    #[test]
    fn a_blend_mixes_the_incoming_frames_into_the_held_tail_and_queues_the_rest() {
        let mut held = Holdback::new(1);
        held.absorb(&[1.0; 10]);
        let mut out = Vec::new();
        assert_eq!(held.begin_mix(4, &mut out), 6);
        assert_eq!(out.len(), 6);

        // Delivered in two pieces, as decoder chunks would be.
        held.absorb(&[0.5; 3]);
        assert_eq!(
            held.release(0, &mut out),
            0,
            "nothing leaves a blend in progress"
        );
        held.absorb(&[0.5; 3]);

        let mut mixed = Vec::new();
        assert_eq!(held.release(0, &mut mixed), 6);
        for (index, &sample) in mixed[..4].iter().enumerate() {
            let (outgoing, incoming) = curves(index, 4);
            assert!(
                (sample - (outgoing + 0.5 * incoming)).abs() < 1e-6,
                "{index}"
            );
        }
        assert_eq!(
            &mixed[4..],
            [0.5, 0.5],
            "after the blend, plain incoming audio"
        );
    }

    #[test]
    fn a_blend_cut_short_fades_the_rest_of_the_tail_out_alone() {
        let mut held = Holdback::new(2);
        held.absorb(&[1.0; 8]);
        let mut out = Vec::new();
        held.begin_mix(4, &mut out);
        held.absorb(&[1.0; 2]);
        held.finish_mix();

        let mut tail = Vec::new();
        held.release(0, &mut tail);
        assert_eq!(tail.len(), 8);
        assert_eq!(
            tail[0], 1.0,
            "the frame that was blended is as it was mixed"
        );
        for frame in 1..4 {
            assert_eq!(tail[frame * 2], curves(frame, 4).0);
            assert_eq!(tail[frame * 2 + 1], curves(frame, 4).0);
        }
    }
}
