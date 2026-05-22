// SPDX-License-Identifier: GPL-3.0-or-later

//! Per-peer sync attempt window.
//!
//! [ADR-0016](../../../../doc/adr/0016-sync-dos-defences.md): three
//! attempts per 60-second window, per peer-id. Mirrors the PAKE
//! rate-limit shape from
//! [ADR-0012](../../../../doc/adr/0012-pake-spake2.md) so users
//! internalise a single "three strikes per minute" rule.
//!
//! State is process-local. The one-shot `bypass sync` path uses it for
//! its own paired-peer attempts; the daemon (Phase 5.2.c) uses it for
//! inbound `WantPackFrom` requests per peer.
//!
//! Time is injected through a [`Clock`] trait so tests don't have to
//! sleep.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Default attempt budget. Three attempts per [`WINDOW`].
pub const MAX_ATTEMPTS: usize = 3;
/// Default rate-limit window.
pub const WINDOW: Duration = Duration::from_secs(60);

/// Source of timestamps. The default uses `Instant::now`; tests
/// substitute a manual clock.
pub trait Clock {
    fn now(&self) -> Instant;
}

/// Production clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Per-peer attempt log. `K` is the peer-id type so this works for both
/// `libp2p::PeerId` (production) and `String` / `&str` (tests).
#[derive(Debug)]
pub struct AttemptLog<K, C = SystemClock>
where
    K: Eq + std::hash::Hash + Clone,
    C: Clock,
{
    buckets: HashMap<K, VecDeque<Instant>>,
    max_attempts: usize,
    window: Duration,
    clock: C,
}

impl<K> AttemptLog<K, SystemClock>
where
    K: Eq + std::hash::Hash + Clone,
{
    /// Construct with the ADR-0016 defaults (3 attempts / 60 s).
    pub fn new() -> Self {
        Self::with_clock(SystemClock)
    }
}

impl<K> Default for AttemptLog<K, SystemClock>
where
    K: Eq + std::hash::Hash + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, C> AttemptLog<K, C>
where
    K: Eq + std::hash::Hash + Clone,
    C: Clock,
{
    /// Construct with a custom clock. Used by tests.
    pub fn with_clock(clock: C) -> Self {
        Self {
            buckets: HashMap::new(),
            max_attempts: MAX_ATTEMPTS,
            window: WINDOW,
            clock,
        }
    }

    /// Override the budget. For tests / future configurability.
    pub fn with_limits(mut self, max_attempts: usize, window: Duration) -> Self {
        self.max_attempts = max_attempts;
        self.window = window;
        self
    }

    /// Test or record one attempt by `peer`. Returns `Ok(())` if the
    /// attempt fits in the budget (in which case it is recorded), or
    /// `Err(RateLimited { retry_after })` if the bucket is full.
    pub fn check_and_record(&mut self, peer: &K) -> Result<(), RateLimited> {
        let now = self.clock.now();
        let cutoff = now.checked_sub(self.window).unwrap_or(now);
        let bucket = self.buckets.entry(peer.clone()).or_default();
        // Prune entries that fell out of the window.
        while bucket.front().is_some_and(|t| *t < cutoff) {
            bucket.pop_front();
        }
        if bucket.len() >= self.max_attempts {
            // The oldest still-live entry's timestamp + window tells the
            // caller how long to wait before this peer fits again.
            let oldest = *bucket.front().expect("bucket non-empty when full");
            let retry_after = (oldest + self.window).saturating_duration_since(now);
            return Err(RateLimited { retry_after });
        }
        bucket.push_back(now);
        Ok(())
    }

    /// How many attempts are currently on the books for `peer`. Useful
    /// for `bypass sync status` (5.2.c) and tests.
    #[cfg(test)]
    fn live_attempts(&mut self, peer: &K) -> usize {
        let now = self.clock.now();
        let cutoff = now.checked_sub(self.window).unwrap_or(now);
        let Some(bucket) = self.buckets.get_mut(peer) else {
            return 0;
        };
        while bucket.front().is_some_and(|t| *t < cutoff) {
            bucket.pop_front();
        }
        bucket.len()
    }
}

/// Returned by [`AttemptLog::check_and_record`] when the budget is
/// exhausted. `retry_after` is a hint, not a guarantee — the caller is
/// free to retry sooner; the next `check_and_record` call will say.
#[derive(Debug, thiserror::Error)]
#[error("rate-limited: retry after {retry_after:?}")]
pub struct RateLimited {
    pub retry_after: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// Manual clock — t starts at an arbitrary base and advances on
    /// each `advance` call.
    #[derive(Debug)]
    struct FakeClock {
        base: Instant,
        offset: Cell<Duration>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                base: Instant::now(),
                offset: Cell::new(Duration::ZERO),
            }
        }

        fn advance(&self, by: Duration) {
            self.offset.set(self.offset.get() + by);
        }
    }

    impl Clock for &FakeClock {
        fn now(&self) -> Instant {
            self.base + self.offset.get()
        }
    }

    #[test]
    fn first_three_attempts_succeed_fourth_is_refused() {
        let clock = FakeClock::new();
        let mut log: AttemptLog<&'static str, _> = AttemptLog::with_clock(&clock);
        for _ in 0..MAX_ATTEMPTS {
            log.check_and_record(&"peer-a").unwrap();
        }
        let err = log.check_and_record(&"peer-a").unwrap_err();
        assert!(
            err.retry_after <= WINDOW,
            "retry_after should be within the window"
        );
    }

    #[test]
    fn budget_is_per_peer() {
        let clock = FakeClock::new();
        let mut log: AttemptLog<&'static str, _> = AttemptLog::with_clock(&clock);
        for _ in 0..MAX_ATTEMPTS {
            log.check_and_record(&"peer-a").unwrap();
        }
        // peer-b has a fresh budget.
        log.check_and_record(&"peer-b").unwrap();
    }

    #[test]
    fn entries_expire_after_window() {
        let clock = FakeClock::new();
        let mut log: AttemptLog<&'static str, _> = AttemptLog::with_clock(&clock);
        for _ in 0..MAX_ATTEMPTS {
            log.check_and_record(&"peer-a").unwrap();
        }
        assert!(log.check_and_record(&"peer-a").is_err());
        // Advance past the window — all three entries fall off.
        clock.advance(WINDOW + Duration::from_secs(1));
        log.check_and_record(&"peer-a").unwrap();
        assert_eq!(log.live_attempts(&"peer-a"), 1);
    }

    #[test]
    fn retry_after_reflects_oldest_entry_age() {
        let clock = FakeClock::new();
        let mut log: AttemptLog<&'static str, _> = AttemptLog::with_clock(&clock);
        log.check_and_record(&"peer-a").unwrap();
        clock.advance(Duration::from_secs(20));
        log.check_and_record(&"peer-a").unwrap();
        clock.advance(Duration::from_secs(20));
        log.check_and_record(&"peer-a").unwrap();
        // Bucket full. Oldest entry is 40s old; should retry in ~20s.
        let err = log.check_and_record(&"peer-a").unwrap_err();
        // Tolerance: anywhere in [19, 21] is fine.
        assert!(
            err.retry_after >= Duration::from_secs(19)
                && err.retry_after <= Duration::from_secs(21),
            "got {:?}",
            err.retry_after
        );
    }

    #[test]
    fn custom_limits_are_honoured() {
        let clock = FakeClock::new();
        let mut log: AttemptLog<&'static str, _> =
            AttemptLog::with_clock(&clock).with_limits(1, Duration::from_secs(5));
        log.check_and_record(&"x").unwrap();
        assert!(log.check_and_record(&"x").is_err());
        clock.advance(Duration::from_secs(6));
        log.check_and_record(&"x").unwrap();
    }
}
