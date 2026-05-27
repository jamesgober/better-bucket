//! Loom concurrency model check for the lock-free acquire path.
//!
//! Compiled and run only under `RUSTFLAGS="--cfg loom" cargo test --test
//! loom_acquire`; under a normal build the `#![cfg(loom)]` gate makes this file
//! empty. Under that cfg the library's state word is a `loom` atomic, so loom
//! exhaustively explores the interleavings of concurrent `try_acquire` calls
//! and checks the safety contract directly on the real `Bucket`.
//!
//! The model uses a bucket whose clock is never advanced, so no refill occurs
//! during the run — the property under test is the CAS itself: with a fixed
//! pool of tokens and more demand than supply, the bucket must grant *exactly*
//! the pool (no over-grant, no lost token) across every interleaving.

#![cfg(loom)]

use better_bucket::Bucket;
use clock_lib::ManualClock;

#[test]
fn loom_acquire_never_over_grants_or_loses_tokens() {
    loom::model(|| {
        // Capacity 2, full, no refill (the manual clock is never advanced).
        let clock = std::sync::Arc::new(ManualClock::new());
        let bucket = loom::sync::Arc::new(Bucket::per_second(2).with_clock(clock));

        let a = loom::sync::Arc::clone(&bucket);
        let b = loom::sync::Arc::clone(&bucket);

        // Each thread demands 2 tokens; combined demand (4) exceeds supply (2).
        let t1 =
            loom::thread::spawn(move || u32::from(a.try_acquire(1)) + u32::from(a.try_acquire(1)));
        let t2 =
            loom::thread::spawn(move || u32::from(b.try_acquire(1)) + u32::from(b.try_acquire(1)));

        let granted = t1.join().unwrap() + t2.join().unwrap();

        // Never over-grant: the two tokens are never handed out more than twice.
        // Never lose a token: with demand exceeding supply, both are handed out.
        assert_eq!(granted, 2, "expected exactly 2 grants, got {granted}");
        assert_eq!(bucket.available(), 0);
    });
}
