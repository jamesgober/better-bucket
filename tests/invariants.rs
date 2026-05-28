//! Property-based invariant suite for the token bucket.
//!
//! These properties encode the crate's two safety contracts and run against the
//! `0.2` simple implementation. They are the same invariants the lock-free
//! `0.3` core must continue to satisfy, so this file moves forward unchanged
//! into that milestone.
//!
//! Gated on `clock`: without it there is no `Bucket` to exercise.

#![cfg(feature = "clock")]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use better_bucket::{Bucket, Decision};
use clock_lib::ManualClock;
use proptest::prelude::*;

proptest! {
    /// Invariant 1: tokens always stay within `[0, capacity]`. No sequence of
    /// acquires and time advances can push the available count above capacity.
    #[test]
    fn tokens_stay_within_capacity(
        rate in 1u32..=10_000,
        steps in proptest::collection::vec((0u32..=20_000u32, 0u64..=5_000u64), 0..64),
    ) {
        let clock = Arc::new(ManualClock::new());
        let bucket = Bucket::per_second(rate).with_clock(Arc::clone(&clock));

        for (take, advance_ms) in steps {
            clock.advance(Duration::from_millis(advance_ms));
            let _ = bucket.try_acquire(take);
            prop_assert!(bucket.available() <= rate);
        }
    }

    /// Invariant 2 (the safety contract): never over-grant. With no time
    /// advancing — hence no refill — the total tokens handed out can never
    /// exceed the initial fill, regardless of the acquire sequence.
    #[test]
    fn never_grants_more_than_available(
        rate in 1u32..=2_000,
        acquires in proptest::collection::vec(1u32..=4_000u32, 0..128),
    ) {
        let clock = Arc::new(ManualClock::new());
        let bucket = Bucket::per_second(rate).with_clock(Arc::clone(&clock));

        let mut granted: u64 = 0;
        for take in acquires {
            if bucket.try_acquire(take) {
                granted += u64::from(take);
            }
        }
        // Started full at `rate`, no refill occurred.
        prop_assert!(granted <= u64::from(rate));
    }

    /// Invariant 2 under refill: total grants never exceed the initial fill
    /// plus what the configured rate could have accrued over the elapsed time.
    #[test]
    fn grants_never_exceed_initial_plus_refill(
        rate in 1u32..=1_000,
        rounds in proptest::collection::vec((1u32..=2_000u32, 0u64..=2_000u64), 0..64),
    ) {
        let clock = Arc::new(ManualClock::new());
        let bucket = Bucket::per_second(rate).with_clock(Arc::clone(&clock));

        let mut granted: u64 = 0;
        let mut elapsed_ms: u64 = 0;
        for (take, advance_ms) in rounds {
            clock.advance(Duration::from_millis(advance_ms));
            elapsed_ms += advance_ms;
            if bucket.try_acquire(take) {
                granted += u64::from(take);
            }
        }

        // Upper bound: the initial full bucket plus rate * elapsed seconds,
        // rounded up. `granted` must never exceed it.
        let accrued = (u128::from(rate) * u128::from(elapsed_ms)).div_ceil(1_000);
        let ceiling = u128::from(rate) + accrued;
        prop_assert!(u128::from(granted) <= ceiling);
    }

    /// The retry hint is honest: after a denial, waiting exactly the reported
    /// `retry_after` must make the same request succeed. This exercises the
    /// fixed-point `time_for` ceiling against the fixed-point refill floor — the
    /// two must agree so the hint never under-promises.
    #[test]
    fn retry_after_is_an_honest_lower_bound(
        rate in 1u32..=10_000,
        take in 1u32..=10_000,
    ) {
        prop_assume!(take <= rate); // n <= capacity, so it is grantable in principle

        let clock = Arc::new(ManualClock::new());
        let bucket = Bucket::per_second(rate).with_clock(Arc::clone(&clock));

        prop_assert!(bucket.try_acquire(rate)); // drain to empty
        if let Decision::Denied { retry_after } = bucket.acquire(take) {
            prop_assume!(retry_after != Duration::MAX);
            clock.advance(retry_after);
            prop_assert!(
                bucket.try_acquire(take),
                "retry_after under-promised: rate={rate}, take={take}, waited {retry_after:?}"
            );
        }
    }
}
