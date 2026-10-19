use std::sync::atomic::{AtomicU32, Ordering};

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
}
