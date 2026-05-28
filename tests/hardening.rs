//! Adversarial inputs and the edge matrix.
//!
//! Exercises the public surface as a consumer would, proving the safety
//! contract under hostile and boundary conditions: no panic, no wrapping or
//! overflow, no over-grant, and tokens always within `[0, capacity]`. Time is
//! driven by a `ManualClock` so the extreme-elapsed cases are deterministic.
//!
//! Gated on `clock`: without it there is no `Bucket` to exercise.

#![cfg(feature = "clock")]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use better_bucket::{Bucket, BucketConfig, Decision};
use clock_lib::ManualClock;

fn manual(rate: u32) -> (Arc<ManualClock>, Bucket<Arc<ManualClock>>) {
    let clock = Arc::new(ManualClock::new());
    let bucket = Bucket::per_second(rate).with_clock(Arc::clone(&clock));
    (clock, bucket)
}

// ---- Adversarial inputs -------------------------------------------------

#[test]
fn test_request_at_u32_max_never_panics_and_is_denied_forever() {
    let (_clock, bucket) = manual(5);
    // Far more than capacity: must be denied with the "never" hint, no overflow.
    assert_eq!(
        bucket.acquire(u32::MAX),
        Decision::Denied {
            retry_after: Duration::MAX
        }
    );
    assert!(!bucket.try_acquire(u32::MAX));
}

#[test]
fn test_capacity_at_u32_max_constructs_and_works() {
    // The packed representation clamps capacity; construction must not panic and
    // the bucket must still grant.
    let bucket = Bucket::per_second(u32::MAX);
    assert!(bucket.capacity() > 0);
    assert!(bucket.capacity() <= 4_294_967); // ~u32::MAX millitokens / 1000
    assert!(bucket.try_acquire(1));
    assert!(bucket.available() <= bucket.capacity());
}

#[test]
fn test_extreme_rate_does_not_overflow() {
    // u32::MAX tokens per nanosecond: the largest plausible refill numerator.
    let (clock, bucket) = {
        let clock = Arc::new(ManualClock::new());
        let bucket =
            Bucket::per_duration(u32::MAX, Duration::from_nanos(1)).with_clock(Arc::clone(&clock));
        (clock, bucket)
    };
    assert!(bucket.try_acquire(1));
    clock.advance(Duration::from_millis(1)); // would accrue an astronomical amount
    assert!(bucket.available() <= bucket.capacity()); // clamped, no wrap
    assert!(bucket.try_acquire(1));
}

#[test]
fn test_near_zero_rate_is_effectively_static() {
    // One token per the largest representable period: refill rounds to nothing.
    let bucket = Bucket::per_duration(1, Duration::MAX);
    assert!(bucket.try_acquire(1)); // the initial token
    assert_eq!(
        bucket.acquire(1),
        Decision::Denied {
            retry_after: Duration::MAX
        }
    );
}

#[test]
fn test_config_with_huge_values_validates() {
    let config = BucketConfig::new(u32::MAX, u32::MAX, Duration::from_nanos(1), u32::MAX).unwrap();
    let bucket = Bucket::from_config(config);
    assert!(bucket.try_acquire(1));
    assert!(bucket.available() <= bucket.capacity());
}

// ---- Edge matrix --------------------------------------------------------

#[test]
fn test_capacity_one_boundaries() {
    let (_clock, bucket) = manual(1);
    assert_eq!(bucket.capacity(), 1);
    assert!(bucket.try_acquire(0)); // n = 0
    assert!(bucket.try_acquire(1)); // n = capacity
    assert!(!bucket.try_acquire(1)); // now empty
    assert_eq!(
        bucket.acquire(2), // n = capacity + 1
        Decision::Denied {
            retry_after: Duration::MAX
        }
    );
    assert!(bucket.try_acquire(0)); // n = 0 still allowed when empty
}

#[test]
fn test_n_around_capacity() {
    let (_clock, bucket) = manual(10);
    assert!(bucket.try_acquire(9)); // capacity - 1
    assert_eq!(bucket.available(), 1);
    assert!(!bucket.try_acquire(2)); // more than what's left
    assert!(bucket.try_acquire(1)); // capacity exact (the last token)
    assert_eq!(bucket.available(), 0);
}

#[test]
fn test_exact_full_then_exact_empty() {
    let (_clock, bucket) = manual(8);
    assert_eq!(bucket.available(), 8); // exact-full
    assert!(bucket.try_acquire(8));
    assert_eq!(bucket.available(), 0); // exact-empty
}

#[test]
fn test_zero_time_delta_does_not_refill() {
    let (_clock, bucket) = manual(100);
    assert!(bucket.try_acquire(100));
    // Repeated reads/acquires with no clock advance: no refill appears.
    for _ in 0..5 {
        assert_eq!(bucket.available(), 0);
        assert!(!bucket.try_acquire(1));
    }
}

#[test]
fn test_clock_not_advancing_holds_steady() {
    let (_clock, bucket) = manual(50);
    assert!(bucket.try_acquire(20));
    let after = bucket.available();
    for _ in 0..10 {
        assert_eq!(bucket.available(), after);
    }
}

// ---- Extreme elapsed durations -----------------------------------------

#[test]
fn test_active_bucket_keeps_refilling_past_the_wrap_window() {
    // The 32-bit millisecond field wraps at ~49.7 days. An actively-used bucket
    // must keep refilling correctly across that boundary — this is the case the
    // old saturating implementation got wrong (it stalled).
    let (clock, bucket) = manual(10);
    assert!(bucket.try_acquire(10)); // drain

    // Three 40-day steps total 120 days, crossing the ~49.7-day wrap.
    for step in 1..=3 {
        clock.advance(Duration::from_secs(40 * 24 * 60 * 60));
        assert_eq!(
            bucket.available(),
            10,
            "refill stalled at step {step} after crossing the wrap window"
        );
        assert!(bucket.try_acquire(10));
    }
}

#[test]
fn test_enormous_single_advance_clamps_without_wrapping() {
    let (clock, bucket) = manual(1_000);
    assert!(bucket.try_acquire(1_000));
    // ~317 years in one jump.
    clock.advance(Duration::from_secs(10_000_000_000));
    let available = bucket.available();
    assert!(available <= bucket.capacity()); // clamped, never wrapped past capacity
    // Still functional afterwards.
    let _ = bucket.try_acquire(1);
    assert!(bucket.available() <= bucket.capacity());
}

// ---- Concurrent reconfiguration ----------------------------------------

#[test]
fn test_concurrent_acquire_and_reset_stay_in_bounds() {
    // Hammer one bucket while another thread repeatedly resets it. `reset` is a
    // deliberate fresh-burst, so total grants are unbounded — but the bucket
    // must never panic, deadlock, or report more than capacity available.
    let bucket = Arc::new(Bucket::per_second(100));
    let granted = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for _ in 0..6 {
        let bucket = Arc::clone(&bucket);
        let granted = Arc::clone(&granted);
        handles.push(thread::spawn(move || {
            for _ in 0..10_000 {
                if bucket.try_acquire(1) {
                    let _ = granted.fetch_add(1, Ordering::Relaxed);
                }
                assert!(bucket.available() <= bucket.capacity());
            }
        }));
    }
    let resetter = {
        let bucket = Arc::clone(&bucket);
        thread::spawn(move || {
            for _ in 0..2_000 {
                bucket.reset();
            }
        })
    };

    for h in handles {
        h.join().unwrap();
    }
    resetter.join().unwrap();
    assert!(bucket.available() <= bucket.capacity());
}
