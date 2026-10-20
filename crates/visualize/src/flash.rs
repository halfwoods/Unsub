//! Photosensitivity: keeps a brightness that follows the music from flashing more than three
//! times a second. [`FlashLimiter`] has the rule.

use std::time::Duration;

/// The smallest swing of brightness (0 to 1) that counts as a change: WCAG's 10% of the maximum
/// relative luminance.
pub const FLASH_THRESHOLD: f32 = 0.1;
/// The most flashes allowed in any one second.
pub const MAX_FLASHES_PER_SECOND: usize = 3;

/// A flash is a rise and a fall.
const CHANGES_PER_FLASH: usize = 2;
/// The changes in any second number the reversals plus the one already under way.
const MAX_REVERSALS: usize = MAX_FLASHES_PER_SECOND * CHANGES_PER_FLASH - 1;
const WINDOW_SECONDS: f64 = 1.0;
/// How close to a full swing a held brightness may come.
const HOLD_FRACTION: f32 = 0.999;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Not yet moved by [`FLASH_THRESHOLD`].
    Unset,
    Rising,
    Falling,
}

/// Limits a brightness series as it arrives, so that it does not flash more than three times a
/// second.
///
/// The rule is WCAG 2.3.1 "Three Flashes or Below Threshold", read the cautious way:
///
/// * A *change* is a swing of brightness of at least [`FLASH_THRESHOLD`] (10% of the full
///   range, the guideline's general flash threshold), in one direction. A rise that takes several
///   steps is still one change; dips smaller than the threshold along the way do not end it.
/// * A *flash* is a pair of opposing changes, a rise and then a fall or the reverse.
/// * No more than [`MAX_FLASHES_PER_SECOND`] flashes in any one second, which is at most six
///   changes in any second, so at most five *reversals* (a change in the other direction from
///   the one before it).
/// * The guideline exempts swings that stay above 80% brightness. That exemption is not taken:
///   every swing counts, however bright.
///
/// Brightness is a number from 0 to 1 standing for relative luminance, so if the UI maps it
/// through a curve or composites it over a picture, the limit should be applied to what reaches
/// the screen. Pure red flashes have a threshold of their own that brightness alone does not
/// capture; avoid flashing saturated red.
///
/// The limiter is a small state machine, written to be copied into the UI line for line.
/// For each sample `(t, wanted)`, with `wanted` clamped to 0..1:
///
/// 1. Until the values seen span [`FLASH_THRESHOLD`], keep their `low` and `high` and show
///    `wanted`. When they first do, the direction is rising if `wanted` is the new high and
///    falling if it is the new low, and `extreme` is `wanted`.
/// 2. When rising: a `wanted` above `extreme` becomes the new `extreme` and is shown. A `wanted`
///    at least the threshold below `extreme` would be a reversal. If fewer than five reversals
///    are recorded in the last second, or the oldest is over a second old, record `t`, turn to
///    falling with `extreme = wanted`, and show `wanted`. Otherwise show `extreme` minus 99.9% of
///    the threshold instead. Anything in between is shown as it is.
/// 3. When falling it is the mirror image.
///
/// The result is that in any second the shown brightness turns round by the threshold or more at
/// most five times, so at most three flashes, whatever is asked for. A request that asks for no
/// more than that is shown exactly as asked.
#[derive(Debug, Clone)]
pub struct FlashLimiter {
    direction: Direction,
    /// The highest (rising) or lowest (falling) value shown in the current direction.
    extreme: f32,
    /// The range of values seen while the direction is unset.
    low: f32,
    high: f32,
    seen: bool,
    /// Times of the latest reversals, as a ring.
    reversals: [f64; MAX_REVERSALS],
    next: usize,
    count: usize,
    last_time: f64,
}

impl Default for FlashLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl FlashLimiter {
    pub fn new() -> Self {
        Self {
            direction: Direction::Unset,
            extreme: 0.0,
            low: 0.0,
            high: 0.0,
            seen: false,
            reversals: [0.0; MAX_REVERSALS],
            next: 0,
            count: 0,
            last_time: 0.0,
        }
    }

    /// Forgets what came before, as when the visualizer is hidden for a while.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The brightness to show at time `t` when `wanted` was asked for. Times should not go
    /// backwards; one that does is treated as the same moment as the last.
    pub fn limit(&mut self, t: Duration, wanted: f32) -> f32 {
        let now = t.as_secs_f64().max(self.last_time);
        self.last_time = now;
        let wanted = if wanted.is_nan() {
            0.0
        } else {
            wanted.clamp(0.0, 1.0)
        };

        match self.direction {
            Direction::Unset => {
                if self.seen {
                    self.low = self.low.min(wanted);
                    self.high = self.high.max(wanted);
                } else {
                    (self.low, self.high, self.seen) = (wanted, wanted, true);
                }
                if self.high - self.low >= FLASH_THRESHOLD {
                    self.direction = if wanted >= self.high {
                        Direction::Rising
                    } else {
                        Direction::Falling
                    };
                    self.extreme = wanted;
                }
                wanted
            }
            Direction::Rising => {
                if wanted > self.extreme {
                    self.extreme = wanted;
                    wanted
                } else if self.extreme - wanted >= FLASH_THRESHOLD {
                    self.reverse(now, wanted, Direction::Falling)
                } else {
                    wanted
                }
            }
            Direction::Falling => {
                if wanted < self.extreme {
                    self.extreme = wanted;
                    wanted
                } else if wanted - self.extreme >= FLASH_THRESHOLD {
                    self.reverse(now, wanted, Direction::Rising)
                } else {
                    wanted
                }
            }
        }
    }

    /// A swing the other way of at least the threshold is asked for: allow it, or hold just short.
    fn reverse(&mut self, now: f64, wanted: f32, to: Direction) -> f32 {
        let oldest = (self.count == MAX_REVERSALS).then(|| self.reversals[self.next]);
        if oldest.is_some_and(|oldest| now - oldest <= WINDOW_SECONDS) {
            let held = FLASH_THRESHOLD * HOLD_FRACTION;
            return match to {
                Direction::Falling => self.extreme - held,
                _ => self.extreme + held,
            };
        }
        self.reversals[self.next] = now;
        self.next = (self.next + 1) % MAX_REVERSALS;
        self.count = (self.count + 1).min(MAX_REVERSALS);
        self.direction = to;
        self.extreme = wanted;
        wanted
    }
}

/// The whole series `(time, wanted brightness)` limited at once. A pure function of its input:
/// the same series always gives the same result.
pub fn limit_flashes(series: &[(Duration, f32)]) -> Vec<f32> {
    let mut limiter = FlashLimiter::new();
    series
        .iter()
        .map(|&(t, wanted)| limiter.limit(t, wanted))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn a_slow_pulse_passes_through_untouched() {
        let mut limiter = FlashLimiter::new();
        for step in 0..40 {
            let wanted = if (step / 10) % 2 == 0 { 0.2 } else { 0.9 };
            assert_eq!(limiter.limit(ms(step * 100), wanted), wanted);
        }
    }

    #[test]
    fn the_reversal_allowance_comes_back_after_a_second() {
        let mut limiter = FlashLimiter::new();
        // Start rising, then reverse five times 100 ms apart.
        limiter.limit(ms(0), 0.0);
        limiter.limit(ms(10), 1.0);
        for reversal in 0..5u64 {
            let wanted = if reversal % 2 == 0 { 0.0 } else { 1.0 };
            assert_eq!(limiter.limit(ms(100 + reversal * 100), wanted), wanted);
        }
        // A sixth reversal inside the second since the first is held short; after it, allowed.
        let held = limiter.limit(ms(700), 1.0);
        assert!(held < 0.2, "{held}");
        assert_eq!(limiter.limit(ms(1101), 1.0), 1.0);
    }
}
