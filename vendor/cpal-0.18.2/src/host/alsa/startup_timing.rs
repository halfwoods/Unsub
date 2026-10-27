//! Bounded compatibility for older ALSA Pulse plugins with missing initial timing data.
use std::time::{Duration, Instant};

pub(crate) const RETRY_INTERVAL: Duration = Duration::from_millis(5);
const RETRY_LIMIT: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(crate) struct StartupTiming {
    first_retry: Option<Instant>,
    measured: bool,
}

impl StartupTiming {
    pub(crate) fn measured(&mut self) {
        self.measured = true;
    }

    pub(crate) fn retry(
        &mut self,
        now: Instant,
        io_error: bool,
        healthy: bool,
        available: Option<i64>,
    ) -> bool {
        if self.measured || !io_error || !healthy || !available.is_some_and(|frames| frames >= 0) {
            return false;
        }
        let first = *self.first_retry.get_or_insert(now);
        now.saturating_duration_since(first) < RETRY_LIMIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_missing_timing_can_recover_without_reopening() {
        let mut timing = StartupTiming::default();
        let now = Instant::now();
        assert!(timing.retry(now, true, true, Some(128)));
        assert!(timing.retry(now + RETRY_INTERVAL, true, true, Some(128)));
        timing.measured();
        assert!(!timing.retry(now + RETRY_INTERVAL * 2, true, true, Some(128)));
    }

    #[test]
    fn repeated_errors_do_not_extend_the_deadline() {
        let mut timing = StartupTiming::default();
        let now = Instant::now();
        assert!(timing.retry(now, true, true, Some(0)));
        assert!(timing.retry(now + RETRY_LIMIT - RETRY_INTERVAL, true, true, Some(0)));
        assert!(!timing.retry(now + RETRY_LIMIT, true, true, Some(0)));
        assert!(!timing.retry(now + RETRY_LIMIT * 2, true, true, Some(0)));
    }

    #[test]
    fn other_errors_and_unhealthy_outputs_are_not_retried() {
        let now = Instant::now();
        for (io_error, healthy, available) in [
            (false, true, Some(128)),
            (true, false, Some(128)),
            (true, true, None),
            (true, true, Some(-1)),
        ] {
            assert!(!StartupTiming::default().retry(now, io_error, healthy, available));
        }
    }

    #[test]
    fn a_successful_first_query_disables_later_retries() {
        let mut timing = StartupTiming::default();
        timing.measured();
        assert!(!timing.retry(Instant::now(), true, true, Some(128)));
    }
}
