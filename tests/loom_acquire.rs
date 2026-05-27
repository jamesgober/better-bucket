//! Loom concurrency model check for `better-bucket`.
//!
//! Compiled and run only under `RUSTFLAGS="--cfg loom" cargo test --test
//! loom_acquire`; under a normal build the `#![cfg(loom)]` gate makes this file
//! empty.
//!
//! `0.1.0` has no acquire path yet, so this model proves two things ahead of
//! the core: that the loom harness compiles and runs against this crate's
//! toolchain, and that the contention mechanic the acquire path will use — a
//! `compare_exchange` loop draining a shared budget — never grants more than
//! the budget allows under any thread interleaving. The full acquire/refill
//! interleaving model (the real no-over-grant proof) replaces this in `0.3.0`.

#![cfg(loom)]

use loom::sync::Arc;
use loom::sync::atomic::{AtomicU64, Ordering};
use loom::thread;

/// Take one unit from a shared budget via CAS, reporting whether it succeeded.
/// This is the bare contention kernel of the future `try_acquire`.
fn try_take_one(state: &AtomicU64) -> bool {
    let mut current = state.load(Ordering::Acquire);
    loop {
        if current == 0 {
            return false;
        }
        match state.compare_exchange_weak(current, current - 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

#[test]
fn loom_cas_take_never_over_grants() {
    loom::model(|| {
        let budget = Arc::new(AtomicU64::new(1));
        let racer = Arc::clone(&budget);

        // One thread races the main thread for a single available unit.
        let handle = thread::spawn(move || try_take_one(&racer));
        let granted_here = try_take_one(&budget);
        let granted_there = handle.join().unwrap();

        // Budget was 1: at most one side may win — never both, never neither
        // when a unit was available to exactly one of them.
        let granted = u64::from(granted_here) + u64::from(granted_there);
        assert!(granted <= 1, "over-grant: budget of 1 handed out {granted}");
        assert_eq!(budget.load(Ordering::Acquire), 1 - granted);
    });
}
