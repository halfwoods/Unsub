//! The flash limiter's guarantee, checked against a measure of flashing written separately from
//! the limiter: find where a brightness series turns, and count the swings between turns.

use std::time::Duration;

use antistreaming_visualize::{
    FLASH_THRESHOLD, FlashLimiter, MAX_FLASHES_PER_SECOND, limit_flashes,
};

/// Where the series turns round, as `(time, value)`, with its first and last sample as bookends.
/// A turn is the highest or lowest value of a run that moved by at least the threshold.
fn turning_points(series: &[(f64, f32)]) -> Vec<(f64, f32)> {
    let mut points = vec![series[0]];
    let (mut high, mut low) = (series[0], series[0]);
    // 1 once a rise of the threshold is seen, -1 once a fall is; 0 before either.
    let mut heading = 0;
    for &(time, value) in &series[1..] {
        match heading {
            0 => {
                if value > high.1 {
                    high = (time, value);
                }
                if value < low.1 {
                    low = (time, value);
                }
                if high.1 - low.1 >= FLASH_THRESHOLD {
                    heading = if high.0 >= low.0 { 1 } else { -1 };
                }
            }
            1 => {
                if value > high.1 {
                    high = (time, value);
                } else if high.1 - value >= FLASH_THRESHOLD {
                    points.push(high);
                    low = (time, value);
                    heading = -1;
                }
            }
            _ => {
                if value < low.1 {
                    low = (time, value);
                } else if value - low.1 >= FLASH_THRESHOLD {
                    points.push(low);
                    high = (time, value);
                    heading = 1;
                }
            }
        }
    }
    points.push(*series.last().unwrap());
    points
}

/// The most complete swings (of the threshold or more) that fit inside any one second.
fn most_swings_in_a_second(series: &[(f64, f32)]) -> usize {
    let points = turning_points(series);
    let mut most = 0;
    let mut first = 0;
    for last in 1..points.len() {
        while points[last].0 - points[first].0 > 1.0 {
            first += 1;
        }
        most = most.max(last - first);
    }
    most
}

fn timed(values: &[f32], rate: f64) -> Vec<(f64, f32)> {
    values
        .iter()
        .enumerate()
        .map(|(n, &value)| (n as f64 / rate, value))
        .collect()
}

fn limited(series: &[(f64, f32)]) -> Vec<(f64, f32)> {
    let asked: Vec<_> = series
        .iter()
        .map(|&(time, value)| (Duration::from_secs_f64(time), value))
        .collect();
    series
        .iter()
        .zip(limit_flashes(&asked))
        .map(|(&(time, _), shown)| (time, shown))
        .collect()
}

struct Random(u64);

impl Random {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

const MOST_SWINGS: usize = MAX_FLASHES_PER_SECOND * 2;

#[test]
fn the_checker_counts_what_a_flash_is() {
    // A square wave at 3 flashes a second is 6 swings in a second.
    let square: Vec<f32> = (0..300)
        .map(|n| if (n / 10) % 2 == 0 { 0.0 } else { 1.0 })
        .collect();
    assert_eq!(most_swings_in_a_second(&timed(&square, 60.0)), 6);
    // Wobbles under the threshold are not changes, nor are the steps of one slow rise.
    let wobble: Vec<f32> = (0..300)
        .map(|n| if n % 2 == 0 { 0.5 } else { 0.58 })
        .collect();
    assert_eq!(most_swings_in_a_second(&timed(&wobble, 60.0)), 0);
    let ramp: Vec<f32> = (0..100).map(|n| n as f32 / 100.0).collect();
    assert_eq!(most_swings_in_a_second(&timed(&ramp, 100.0)), 1);
}

#[test]
fn whatever_is_asked_no_second_holds_more_than_three_flashes() {
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    for case in 0..400 {
        let rate = 30.0 + f64::from(random.next()) * 210.0;
        let length = 100 + (random.next() * 900.0) as usize;
        let kind = case % 5;
        let beat = 1.0 + random.next() * 15.0;
        let depth = 0.05 + random.next() * 0.95;
        let mut level = 0.5_f32;
        let asked: Vec<f32> = (0..length)
            .map(|n| {
                let t = n as f32 / rate as f32;
                match kind {
                    // A square wave at a random beat, and a triangle one.
                    0 => 0.5 + 0.5 * depth * (std::f32::consts::TAU * beat * t).sin().signum(),
                    1 => 0.5 + depth * ((beat * t).fract() - 0.5).abs() * 2.0 - depth / 2.0,
                    // Noise, a random walk, and steps of random length.
                    2 => random.next(),
                    3 => {
                        level = (level + (random.next() - 0.5) * 0.4).clamp(0.0, 1.0);
                        level
                    }
                    _ => {
                        if random.next() < beat / rate as f32 * 2.0 {
                            level = random.next();
                        }
                        level
                    }
                }
            })
            .collect();
        let series = timed(&asked, rate);

        let shown = limited(&series);

        let swings = most_swings_in_a_second(&shown);
        assert!(
            swings <= MOST_SWINGS,
            "case {case} (kind {kind}, {rate:.0} Hz, beat {beat:.1}): {swings} swings"
        );
        assert!(shown.iter().all(|&(_, value)| (0.0..=1.0).contains(&value)));
    }
}

#[test]
fn anything_already_within_the_limit_passes_through_unchanged() {
    // A slow sine, a 2-flash-a-second pulse and a rise and fall every second.
    let slow: Vec<f32> = (0..600)
        .map(|n| 0.5 + 0.5 * (std::f32::consts::TAU * 0.7 * n as f32 / 120.0).sin())
        .collect();
    let pulse: Vec<f32> = (0..600)
        .map(|n| if (n / 30) % 2 == 0 { 0.1 } else { 0.9 })
        .collect();
    let small: Vec<f32> = (0..600)
        .map(|n| if n % 3 == 0 { 0.4 } else { 0.48 })
        .collect();
    for asked in [slow, pulse, small] {
        let series = timed(&asked, 120.0);
        assert!(most_swings_in_a_second(&series) <= MOST_SWINGS);

        let shown: Vec<f32> = limited(&series).into_iter().map(|(_, v)| v).collect();

        assert_eq!(shown, asked);
    }
}

#[test]
fn a_strobe_is_cut_down_and_the_limit_lifts_when_it_stops() {
    let mut limiter = FlashLimiter::new();
    let rate = 100.0;
    let mut shown = Vec::new();
    for n in 0..300 {
        let asked = if (n / 3) % 2 == 0 { 0.0 } else { 1.0 };
        shown.push((
            n as f64 / rate,
            limiter.limit(Duration::from_secs_f64(n as f64 / rate), asked),
        ));
    }
    assert!(most_swings_in_a_second(&shown) <= MOST_SWINGS);
    assert!(
        shown.iter().any(|&(_, v)| v > 0.9) && shown.iter().any(|&(_, v)| v < 0.1),
        "some flashes still show"
    );

    // A calm second later, a single swing is shown whole.
    let later = Duration::from_secs_f64(3.0 + 1.2);
    let calm = limiter.limit(later, 0.0);
    let swung = limiter.limit(later + Duration::from_millis(10), 1.0);
    let back = limiter.limit(later + Duration::from_millis(20), 0.0);
    assert_eq!((swung, back), (1.0, 0.0), "{calm}");
}

#[test]
fn a_reset_forgets_the_flashes_and_odd_input_is_tamed() {
    let mut limiter = FlashLimiter::new();
    for n in 0..20 {
        limiter.limit(Duration::from_millis(n * 10), (n % 2) as f32);
    }
    limiter.reset();
    assert_eq!(limiter.limit(Duration::from_millis(0), 0.0), 0.0);
    assert_eq!(limiter.limit(Duration::from_millis(10), 1.0), 1.0);

    let mut odd = FlashLimiter::new();
    assert_eq!(odd.limit(Duration::ZERO, f32::NAN), 0.0);
    assert_eq!(odd.limit(Duration::from_millis(10), 7.0), 1.0);
    assert_eq!(odd.limit(Duration::from_millis(20), -3.0), 0.0);
    // Time running backwards is no escape.
    for n in 0..30u64 {
        let shown = odd.limit(Duration::from_millis(20 - n.min(20)), (n % 2) as f32);
        assert!((0.0..=1.0).contains(&shown));
    }
}

#[test]
fn the_limiter_is_deterministic() {
    let series: Vec<(Duration, f32)> = (0..200)
        .map(|n| (Duration::from_millis(n * 7), ((n * 37) % 11) as f32 / 10.0))
        .collect();
    assert_eq!(limit_flashes(&series), limit_flashes(&series));
}
