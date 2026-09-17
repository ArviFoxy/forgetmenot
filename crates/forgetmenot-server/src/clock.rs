//! The server's source of time.
//!
//! Every timestamp the server records goes through a `Clock` so that tests can
//! pin time: a context's `last_seen` and a statistics row's `ts` are then
//! inputs of the test rather than of the moment it ran.

use std::sync::Mutex;

use chrono::{DateTime, Duration, TimeZone, Utc};

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

/// A clock that stands still until a test moves it, for the tests that need two
/// events to fall at instants they choose rather than at the same one.
#[derive(Debug)]
pub struct ManualClock {
    instant: Mutex<DateTime<Utc>>,
}

impl ManualClock {
    pub fn new(instant: DateTime<Utc>) -> Self {
        Self {
            instant: Mutex::new(instant),
        }
    }

    /// Move the clock forward, so that whatever happens next is recorded later
    /// than everything before it.
    pub fn advance(&self, by: Duration) {
        let mut instant = self.locked();
        *instant += by;
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, DateTime<Utc>> {
        self.instant
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for ManualClock {
    /// The same instant a [`FixedClock`] starts from, so that a test reads the
    /// same timestamps until it moves the clock itself.
    fn default() -> Self {
        Self::new(FixedClock::at_epoch_day().now())
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.locked()
    }
}
