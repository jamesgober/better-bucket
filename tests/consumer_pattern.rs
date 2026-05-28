//! First-consumer shake-out.
//!
//! Exercises the public surface the way a downstream limiter (e.g. `rate-net`)
//! will: code against the [`TokenBucket`] trait rather than the concrete type,
//! inject a shared clock for deterministic tests, and turn a [`Decision`] into a
//! consumer's own allow/deny with a retry hint. The point is to surface any API
//! friction before the real integration; if these read naturally, the surface
//! is consumable.
//!
//! Gated on `clock`: without it there is no `Bucket` to exercise.

#![cfg(feature = "clock")]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use better_bucket::{Bucket, BucketConfig, Decision, TokenBucket};
use clock_lib::ManualClock;

/// The kind of allow/deny a gatekeeper exposes — the shape `rate-net` returns to
/// its callers. Built purely from what the [`TokenBucket`] trait provides.
#[derive(Debug, PartialEq, Eq)]
enum Admission {
    Allow,
    Deny { retry_after: Duration },
}

/// A consumer's `check`: depend on the trait, not the concrete clock type.
fn check(bucket: &dyn TokenBucket, cost: u32) -> Admission {
    match bucket.acquire(cost) {
        Decision::Allowed => Admission::Allow,
        Decision::Denied { retry_after } => Admission::Deny { retry_after },
        _ => Admission::Deny {
            retry_after: Duration::MAX,
        },
    }
}

#[test]
fn test_consumer_checks_through_the_trait() {
    let clock = Arc::new(ManualClock::new());
    let bucket = Bucket::per_second(2).with_clock(Arc::clone(&clock));

    // Two allowed, then denied with a usable retry hint.
    assert_eq!(check(&bucket, 1), Admission::Allow);
    assert_eq!(check(&bucket, 1), Admission::Allow);
    assert_eq!(
        check(&bucket, 1),
        Admission::Deny {
            retry_after: Duration::from_millis(500)
        }
    );

    // Wait out the retry hint, then it's allowed again — the hint is honest.
    clock.advance(Duration::from_millis(500));
    assert_eq!(check(&bucket, 1), Admission::Allow);
}

#[test]
fn test_consumer_holds_buckets_with_different_clocks_behind_the_trait() {
    // A consumer may use the system clock in production and a manual clock in
    // tests; both must be usable through the same `&dyn TokenBucket`.
    let system = Bucket::per_second(4);
    let manual = Bucket::per_second(4).with_clock(Arc::new(ManualClock::new()));

    let limiters: [&dyn TokenBucket; 2] = [&system, &manual];
    for limiter in limiters {
        assert_eq!(limiter.capacity(), 4);
        assert!(limiter.try_acquire(4));
        assert!(!limiter.try_acquire(1));
    }
}

#[test]
fn test_consumer_per_key_buckets_share_one_clock() {
    // The keyed pattern: a bucket per key, all sharing a single injected clock,
    // so a test can advance time once and have every limiter see it. (The keyed
    // store itself is the consumer's concern; here we just hold a few buckets.)
    let clock = Arc::new(ManualClock::new());
    let make = |rate: u32| Bucket::per_second(rate).with_clock(Arc::clone(&clock));

    let alice = make(3);
    let bob = make(1);

    assert!(alice.try_acquire(3));
    assert!(bob.try_acquire(1));
    assert!(!alice.try_acquire(1));
    assert!(!bob.try_acquire(1));

    // One advance refills every key's bucket deterministically.
    clock.advance(Duration::from_secs(1));
    assert_eq!(alice.available(), 3);
    assert_eq!(bob.available(), 1);
}

#[test]
fn test_consumer_configures_via_config_then_injects_clock() {
    // The configured construction path a consumer uses for non-default quotas.
    let clock = Arc::new(ManualClock::new());
    let config = BucketConfig::new(100, 10, Duration::from_secs(1), 0).unwrap();
    let bucket = Bucket::from_config(config).with_clock(Arc::clone(&clock));

    assert_eq!(bucket.available(), 0); // started empty per the config
    clock.advance(Duration::from_secs(1));
    assert_eq!(bucket.available(), 10); // one second of refill at 10/s
}
