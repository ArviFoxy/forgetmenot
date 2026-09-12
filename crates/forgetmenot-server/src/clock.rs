//! The server's source of time.
//!
//! Every timestamp the server records goes through a `Clock` so that tests can
//! pin time: a context's `last_seen` and a statistics row's `ts` are then
//! inputs of the test rather than of the moment it ran.

use chrono::{DateTime, TimeZone, Utc};

/// What the server asks for the current time.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// The wall clock, used in production.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that always reports the same instant.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock {
    instant: DateTime<Utc>,
}

impl FixedClock {
    pub fn new(instant: DateTime<Utc>) -> Self {
        Self { instant }
    }

    /// A fixed clock at a round instant, for tests that only need time to stand
    /// still and do not care which instant it is.
    pub fn at_epoch_day() -> Self {
        Self::new(
            Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
                .single()
                .expect("a valid instant"),
        )
    }
}

impl Default for FixedClock {
    fn default() -> Self {
        Self::at_epoch_day()
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.instant
    }
}
