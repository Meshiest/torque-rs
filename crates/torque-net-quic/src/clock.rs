//! The clock the endpoints read: the system's for real sockets, and whatever a test implements [`Clock`] with to step time by hand
//! (the tests here wrap `torque_testkit::FakeClock`).

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use quinn_proto::{StdSystemTime, TimeSource};

/// Where an endpoint reads the time: every `now` quinn-proto is handed, and the wall-clock source its tokens are stamped by.
pub trait Clock {
    fn now(&self) -> Instant;
    /// The wall clock: what certificates are dated by and rotated on.
    fn wall(&self) -> SystemTime;
    /// The wall-clock source the server's handshake tokens are stamped and checked by (the same clock, so a token's age
    /// moves with the test's steps).
    fn time_source(&self) -> Arc<dyn TimeSource>;
}

/// The operating system's clocks.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }

    fn time_source(&self) -> Arc<dyn TimeSource> {
        Arc::new(StdSystemTime)
    }
}
