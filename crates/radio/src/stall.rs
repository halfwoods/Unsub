//! A per-read timeout for ureq, which only offers whole-body budgets. The same connector the
//! podcasts and remote crates use; a live stream is the extreme case, where a request may last for
//! hours and only silence is a fault.
//!
//! Every wait for incoming bytes is capped at the stall timeout, so a transfer may take as long
//! as it likes provided data keeps arriving.

use std::time::Duration;

use ureq::Timeout;
use ureq::unversioned::transport::time::Duration as TimeoutDuration;
use ureq::unversioned::transport::{Buffers, ConnectionDetails, Connector, NextTimeout, Transport};

#[derive(Debug)]
pub(crate) struct StallConnector {
    timeout: Duration,
}

impl StallConnector {
    pub fn new(timeout: Duration) -> StallConnector {
        StallConnector { timeout }
    }
}

impl<In: Transport> Connector<In> for StallConnector {
    type Out = StallTransport<In>;

    fn connect(
        &self,
        _details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(|inner| StallTransport {
            inner,
            timeout: self.timeout,
        }))
    }
}

#[derive(Debug)]
pub(crate) struct StallTransport<T> {
    inner: T,
    timeout: Duration,
}

impl<T: Transport> Transport for StallTransport<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.inner.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        self.inner.await_input(cap(timeout, self.timeout))
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}

/// Keeps ureq's own deadline when it is sooner, so a configured request timeout still wins.
fn cap(timeout: NextTimeout, stall: Duration) -> NextTimeout {
    if *timeout.after <= stall {
        timeout
    } else {
        NextTimeout {
            after: TimeoutDuration::Exact(stall),
            reason: Timeout::RecvBody,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STALL: Duration = Duration::from_millis(200);

    #[test]
    fn unbounded_waits_are_capped() {
        let unbounded = NextTimeout {
            after: TimeoutDuration::NotHappening,
            reason: Timeout::Global,
        };
        let capped = cap(unbounded, STALL);
        assert_eq!(*capped.after, STALL);
        assert_eq!(capped.reason, Timeout::RecvBody);
    }

    #[test]
    fn sooner_deadlines_are_kept() {
        let soon = NextTimeout {
            after: TimeoutDuration::Exact(Duration::from_millis(50)),
            reason: Timeout::Global,
        };
        assert_eq!(cap(soon, STALL), soon);
    }
}
