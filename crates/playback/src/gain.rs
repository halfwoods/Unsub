use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

/// Time a full-scale gain change takes; short enough to feel instant, long enough to avoid clicks.
const RAMP_SECONDS: f32 = 0.010;

/// User volume in `0.0..=1.0`, shared lock-free between the control side and the audio callback.
pub struct Volume(AtomicU32);

impl Volume {
    pub fn new(volume: f32) -> Self {
        Volume(AtomicU32::new(sanitize(volume).to_bits()))
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, volume: f32) {
        self.0.store(sanitize(volume).to_bits(), Ordering::Relaxed);
    }
}

fn sanitize(volume: f32) -> f32 {
    if volume.is_nan() {
        0.0
    } else {
        volume.clamp(0.0, 1.0)
    }
}

/// Highest true peak a track may reach once Sound Check has adjusted it. There is no limiter
/// behind it, and a little headroom keeps the converter from clipping on inter-sample overs.
const CEILING_DBTP: f32 = -1.0;

/// Sound Check's adjustment for one track, from its measured loudness.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gain {
    /// The change that brings the track to the target loudness; positive boosts.
    pub db: f32,
    /// The track's own true peak in dBTP, which limits how far it can be boosted.
    pub peak_dbtp: f32,
}

impl Gain {
    /// What is really applied: a boost is held back so that the adjusted peak stays at or under
    /// -1 dBTP, down to no boost at all for a track that already peaks above that; a boost never
    /// becomes a cut. A cut is applied as asked. A gain that is not a number is ignored rather
    /// than passed on to the speakers.
    pub fn applied_db(self) -> f32 {
        if !self.db.is_finite() || self.peak_dbtp.is_nan() {
            return 0.0;
        }
        if self.db > 0.0 {
            self.db.min((CEILING_DBTP - self.peak_dbtp).max(0.0))
        } else {
            self.db
        }
    }

    pub(crate) fn amplitude(self) -> f32 {
        10f32.powf(self.applied_db() / 20.0)
    }
}

/// Maps the slider position to amplitude. Loudness is perceived roughly logarithmically, so a
/// cubic taper keeps the quiet half of the slider usable where a linear one would be all loud.
pub fn curve(volume: f32) -> f32 {
    volume * volume * volume
}

/// Slews a gain toward its target at a fixed rate so that no change is ever a step.
pub struct Ramp {
    gain: f32,
    step: f32,
}

impl Ramp {
    pub fn new(sample_rate: u32, initial: f32) -> Self {
        Ramp {
            gain: initial,
            step: 1.0 / (sample_rate as f32 * RAMP_SECONDS),
        }
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Advances one frame. Lands on `target` exactly, so steady state is bit-exact.
    pub fn next(&mut self, target: f32) -> f32 {
        let delta = target - self.gain;
        self.gain = if delta.abs() <= self.step {
            target
        } else {
            self.gain + self.step.copysign(delta)
        };
        self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_is_silent_at_zero_unity_at_full_and_monotonic() {
        assert_eq!(curve(0.0), 0.0);
        assert_eq!(curve(1.0), 1.0);
        assert!((curve(0.5) - 0.125).abs() < 1e-6);
        assert!((0..100).all(|n| curve(n as f32 / 100.0) < curve((n + 1) as f32 / 100.0)));
    }

    #[test]
    fn volume_clamps_and_rejects_nan() {
        let volume = Volume::new(0.4);
        assert_eq!(volume.get(), 0.4);
        volume.set(3.0);
        assert_eq!(volume.get(), 1.0);
        volume.set(-1.0);
        assert_eq!(volume.get(), 0.0);
        volume.set(f32::NAN);
        assert_eq!(volume.get(), 0.0);
    }

    #[test]
    fn ramp_takes_ten_milliseconds_for_full_scale() {
        let mut ramp = Ramp::new(48_000, 0.0);
        let frames = (1..).find(|_| ramp.next(1.0) == 1.0);
        assert!(matches!(frames, Some(480..=481)), "{frames:?}");
    }

    #[test]
    fn ramp_never_steps_by_more_than_its_slew_rate() {
        let mut ramp = Ramp::new(44_100, 1.0);
        let mut previous = ramp.gain();
        for _ in 0..1_000 {
            let gain = ramp.next(0.0);
            assert!((previous - gain).abs() <= 1.0 / 441.0 + 1e-6);
            previous = gain;
        }
        assert_eq!(previous, 0.0);
    }

    #[test]
    fn ramp_holds_target_exactly() {
        let mut ramp = Ramp::new(44_100, 0.0);
        for _ in 0..1_000 {
            ramp.next(0.3);
        }
        assert_eq!(ramp.gain(), 0.3);
    }

    #[test]
    fn a_boost_stops_where_the_peak_would_pass_minus_one_dbtp() {
        let gain = |db, peak_dbtp| Gain { db, peak_dbtp }.applied_db();
        assert_eq!(gain(12.0, -6.0), 5.0);
        assert_eq!(gain(3.0, -6.0), 3.0, "a boost with room is left alone");
        assert_eq!(gain(-8.0, -0.1), -8.0, "a cut never needs holding back");
        assert_eq!(gain(2.0, -0.5), 0.0, "no room means no boost, not a cut");
        assert_eq!(
            gain(2.0, 0.3),
            0.0,
            "even for a peak already over full scale"
        );
        assert_eq!(gain(6.0, f32::NEG_INFINITY), 6.0, "silence cannot clip");
    }

    #[test]
    fn a_gain_that_is_not_a_number_is_not_applied() {
        for (db, peak_dbtp) in [(f32::NAN, -6.0), (f32::INFINITY, -6.0), (3.0, f32::NAN)] {
            let gain = Gain { db, peak_dbtp };
            assert_eq!(gain.applied_db(), 0.0, "{db} {peak_dbtp}");
            assert_eq!(gain.amplitude(), 1.0);
        }
    }

    #[test]
    fn amplitude_is_the_decibel_gain_as_a_ratio() {
        let amplitude = |db| {
            Gain {
                db,
                peak_dbtp: -30.0,
            }
            .amplitude()
        };
        assert!((amplitude(6.0206) - 2.0).abs() < 1e-4);
        assert!((amplitude(-20.0) - 0.1).abs() < 1e-6);
        assert_eq!(amplitude(0.0), 1.0);
    }

    #[test]
    fn gain_reads_camel_case_json() {
        let gain: Gain = serde_json::from_str(r#"{"db": -3.5, "peakDbtp": -0.2}"#).unwrap();
        assert_eq!(
            gain,
            Gain {
                db: -3.5,
                peak_dbtp: -0.2
            }
        );
    }
}
