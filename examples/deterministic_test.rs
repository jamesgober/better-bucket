//! Deterministic, sleep-free testing with an injected `ManualClock`.
//!
//! This is the pattern to copy into your own test suite: share a `ManualClock`
//! with the bucket, drive time forward by hand, and assert exact token counts —
//! no real time passes, so the test is fast and never flaky. Run with
//! `cargo run --example deterministic_test`.

use std::sync::Arc;
use std::time::Duration;

use better_bucket::{Bucket, Decision};
use clock_lib::ManualClock;

fn main() {
    let clock = Arc::new(ManualClock::new());
    let bucket = Bucket::per_second(2).with_clock(Arc::clone(&clock));

    // Drain the bucket, then observe a denial with its retry hint.
    assert!(bucket.try_acquire(2));
    match bucket.acquire(1) {
        Decision::Denied { retry_after } => {
            println!("denied; retry after {retry_after:?}");
            assert_eq!(retry_after, Duration::from_millis(500));
        }
        other => println!("unexpected: {other:?}"),
    }

    // Advance half a second: one token accrues at 2/second.
    clock.advance(Duration::from_millis(500));
    assert_eq!(bucket.available(), 1);
    assert!(bucket.try_acquire(1));

    println!("deterministic refill verified without sleeping");
}
