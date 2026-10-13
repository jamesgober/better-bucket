//! The `metrics` feature: decision counters on `Bucket` and `BucketCore`.

#![cfg(feature = "metrics")]
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use better_bucket::{BucketConfig, BucketCore, BucketMetrics};

fn core(capacity: u32) -> BucketCore {
    BucketCore::new(
        BucketConfig::new(capacity, capacity, Duration::from_secs(1), capacity).unwrap(),
        Duration::ZERO,
    )
}

#[test]
fn test_metrics_start_at_zero() {
    assert_eq!(core(5).metrics(), BucketMetrics::default());
}

#[test]
fn test_metrics_count_every_decision_kind() {
    let bucket = core(10);
    let t = Duration::ZERO;
    assert!(bucket.try_acquire_at(4, t)); // allowed, 4
    assert!(bucket.try_acquire_at(0, t)); // allowed, 0
    assert!(!bucket.try_acquire_at(11, t)); // denied (over capacity)
    assert!(!bucket.try_acquire_at(7, t)); // denied (short)
    assert_eq!(bucket.acquire_up_to_at(10, t), 6); // allowed, 6
    assert_eq!(bucket.acquire_up_to_at(10, t), 0); // denied
    assert!(bucket.reserve_at(5, Duration::from_secs(1), t).is_some()); // allowed, 5
    assert!(bucket.reserve_at(10, Duration::ZERO, t).is_none()); // denied
    bucket.refund(3); // not a decision
    bucket.reset_at(t); // not a decision
    let _ = bucket.available_at(t); // not a decision
    let _ = bucket.time_until_at(1, t); // not a decision

    let metrics = bucket.metrics();
    assert_eq!(metrics.allowed, 4);
    assert_eq!(metrics.denied, 4);
    assert_eq!(metrics.tokens_granted, 15);
}

#[cfg(feature = "clock")]
#[test]
fn test_bucket_metrics_match_decisions_under_threads() {
    use better_bucket::Bucket;
    use clock_lib::ManualClock;
    use std::sync::Arc;
    use std::thread;

    let bucket = Arc::new(Bucket::per_second(1_000).with_clock(Arc::new(ManualClock::new())));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let bucket = Arc::clone(&bucket);
            thread::spawn(move || (0..500).filter(|_| bucket.try_acquire(1)).count() as u64)
        })
        .collect();
    let granted: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    let metrics = bucket.metrics();
    assert_eq!(granted, 1_000);
    assert_eq!(metrics.allowed, 1_000);
    assert_eq!(metrics.tokens_granted, 1_000);
    assert_eq!(metrics.denied, 3_000);
}
