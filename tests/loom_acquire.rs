//! Loom concurrency model checks for the lock-free acquire path.
//!
//! Compiled and run only under `RUSTFLAGS="--cfg loom" cargo test --test
//! loom_acquire`; under a normal build the `#![cfg(loom)]` gate makes this file
//! empty. Under that cfg the library's state word is a `loom` atomic, so loom
//! exhaustively explores the interleavings of concurrent calls and checks the
//! safety contract directly on the real `Bucket` and `BucketCore`.
//!
//! The clock matters as much as the CAS. 1.0.0's model froze the clock, and the
//! stale-timestamp over-grant it missed needs a clock that moves while another
//! thread is mid-acquire. Here the clock is itself a loom atomic (an index into
//! pre-captured monotonic readings), so loom also explores every ordering of
//! clock reads and clock advances against the state-word CAS.

#![cfg(loom)]
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use better_bucket::{Bucket, BucketConfig, BucketCore};
use clock_lib::{Clock, ManualClock, Monotonic, Wall};
use loom::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// A monotonic clock whose "now" is a loom atomic, so loom schedules clock
/// reads and advances like any other shared access. Each `advance` moves to the
/// next pre-captured reading, `step` apart.
struct LoomClock {
    readings: Vec<Monotonic>,
    index: AtomicUsize,
    wall: Wall,
}

impl LoomClock {
    fn new(steps: usize, step: Duration) -> Self {
        let manual = ManualClock::new();
        let mut readings = Vec::with_capacity(steps + 1);
        for _ in 0..=steps {
            readings.push(manual.now());
            manual.advance(step);
        }
        Self {
            readings,
            index: AtomicUsize::new(0),
            wall: manual.wall(),
        }
    }

    fn advance(&self) {
        let _ = self.index.fetch_add(1, Ordering::SeqCst);
    }
}

impl Clock for LoomClock {
    fn now(&self) -> Monotonic {
        let index = self.index.load(Ordering::SeqCst);
        self.readings[index.min(self.readings.len() - 1)]
    }

    fn wall(&self) -> Wall {
        self.wall
    }
}

fn per_second(rate: u32, initial: u32) -> BucketConfig {
    BucketConfig::new(rate, rate, Duration::from_secs(1), initial).unwrap()
}

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

#[test]
fn loom_advancing_clock_never_over_grants() {
    // The BB-COR-01 shape: one thread reads the clock and is then overtaken by
    // a thread that advances the clock and commits a later timestamp. 1.0.0
    // read the stale reading as a ~49.7-day gap and granted 3 of 2 tokens.
    loom::model(|| {
        let clock = std::sync::Arc::new(LoomClock::new(1, Duration::from_millis(1)));
        let bucket = loom::sync::Arc::new(
            Bucket::from_config(per_second(2, 2)).with_clock(std::sync::Arc::clone(&clock)),
        );

        let a = loom::sync::Arc::clone(&bucket);
        let b = loom::sync::Arc::clone(&bucket);
        let advancing = std::sync::Arc::clone(&clock);

        let t1 = loom::thread::spawn(move || u32::from(a.try_acquire(1)));
        let t2 = loom::thread::spawn(move || {
            advancing.advance();
            u32::from(b.try_acquire(2)) * 2
        });
        let granted = t1.join().unwrap() + t2.join().unwrap();

        // Capacity 2 plus 1 ms at 2/s (0.002 tokens): at most 2 whole tokens,
        // and whatever is left must not exceed what was not granted.
        assert!(granted <= 2, "over-grant: {granted} tokens granted (max 2)");
        assert!(granted >= 1, "both requests denied from a full bucket");
        assert!(granted + bucket.available() <= 2);
    });
}

#[test]
fn loom_stale_reader_with_two_advances_never_over_grants() {
    // Two clock advances interleaved with three acquires across two threads;
    // every interleaving must respect capacity + accrued refill. At 1000/s a
    // 1 ms step accrues exactly one token.
    loom::model(|| {
        let clock = std::sync::Arc::new(LoomClock::new(2, Duration::from_millis(1)));
        let config = BucketConfig::new(2, 1_000, Duration::from_secs(1), 2).unwrap();
        let bucket = loom::sync::Arc::new(
            Bucket::from_config(config).with_clock(std::sync::Arc::clone(&clock)),
        );

        let a = loom::sync::Arc::clone(&bucket);
        let b = loom::sync::Arc::clone(&bucket);
        let advancing = std::sync::Arc::clone(&clock);

        let t1 =
            loom::thread::spawn(move || u32::from(a.try_acquire(1)) + u32::from(a.try_acquire(1)));
        let t2 = loom::thread::spawn(move || {
            advancing.advance();
            let first = u32::from(b.try_acquire(1));
            advancing.advance();
            first + u32::from(b.try_acquire(1))
        });
        let granted = t1.join().unwrap() + t2.join().unwrap();

        // 2 initial + 2 ms × 1 token/ms.
        assert!(granted <= 4, "over-grant: {granted} > 4");
        assert!(granted + bucket.available() <= 4);
    });
}

#[test]
fn loom_core_with_caller_time_never_over_grants() {
    // The caller-driven path: time is a shared atomic the threads read
    // themselves, so a reader can act on a value another thread has already
    // advanced past.
    loom::model(|| {
        let bucket = loom::sync::Arc::new(BucketCore::new(per_second(2, 2), Duration::ZERO));
        let now_ms = loom::sync::Arc::new(AtomicU64::new(0));

        let a = loom::sync::Arc::clone(&bucket);
        let b = loom::sync::Arc::clone(&bucket);
        let time_a = loom::sync::Arc::clone(&now_ms);
        let time_b = loom::sync::Arc::clone(&now_ms);

        let t1 = loom::thread::spawn(move || {
            let now = Duration::from_millis(time_a.load(Ordering::SeqCst));
            u32::from(a.try_acquire_at(1, now))
        });
        let t2 = loom::thread::spawn(move || {
            let _ = time_b.fetch_add(1, Ordering::SeqCst);
            let now = Duration::from_millis(time_b.load(Ordering::SeqCst));
            u32::from(b.try_acquire_at(2, now)) * 2
        });
        let granted = t1.join().unwrap() + t2.join().unwrap();

        assert!(granted <= 2, "over-grant: {granted} > 2");
        assert!(granted + bucket.available_at(Duration::from_millis(1)) <= 2);
    });
}

#[test]
fn loom_reset_racing_acquire_is_bounded() {
    // `reset` grants one fresh burst by design, so with one reset the total is
    // bounded by two bursts — and the state must never corrupt.
    loom::model(|| {
        let clock = std::sync::Arc::new(LoomClock::new(1, Duration::from_millis(1)));
        let bucket = loom::sync::Arc::new(
            Bucket::from_config(per_second(2, 2)).with_clock(std::sync::Arc::clone(&clock)),
        );

        let a = loom::sync::Arc::clone(&bucket);
        let b = loom::sync::Arc::clone(&bucket);
        let advancing = std::sync::Arc::clone(&clock);

        let t1 = loom::thread::spawn(move || {
            u32::from(a.try_acquire(2)) * 2 + u32::from(a.try_acquire(1))
        });
        let t2 = loom::thread::spawn(move || {
            advancing.advance();
            b.reset();
        });
        let granted = t1.join().unwrap();
        t2.join().unwrap();

        assert!(granted <= 4, "over-grant: {granted} > 2 bursts");
        assert!(bucket.available() <= bucket.capacity());
    });
}

#[test]
fn loom_refund_racing_acquire_conserves_tokens() {
    // A refund returns exactly what was taken: with a frozen clock, the tokens
    // still held at the end plus those available must equal the capacity.
    loom::model(|| {
        let bucket = loom::sync::Arc::new(BucketCore::new(per_second(2, 2), Duration::ZERO));
        let a = loom::sync::Arc::clone(&bucket);
        let b = loom::sync::Arc::clone(&bucket);

        let t1 = loom::thread::spawn(move || {
            if a.try_acquire_at(2, Duration::ZERO) {
                a.refund(2);
            }
            0_u32
        });
        let t2 = loom::thread::spawn(move || u32::from(b.try_acquire_at(1, Duration::ZERO)));
        let held = t1.join().unwrap() + t2.join().unwrap();

        assert_eq!(held + bucket.available_at(Duration::ZERO), 2);
    });
}

#[test]
fn loom_reservation_and_acquire_agree_on_order() {
    // A reservation queues behind tokens already granted, and a grant cannot
    // jump a reservation that already took the tokens.
    loom::model(|| {
        let bucket = loom::sync::Arc::new(BucketCore::new(per_second(2, 2), Duration::ZERO));
        let a = loom::sync::Arc::clone(&bucket);
        let b = loom::sync::Arc::clone(&bucket);

        let t1 = loom::thread::spawn(move || a.reserve_at(2, Duration::MAX, Duration::ZERO));
        let t2 = loom::thread::spawn(move || b.try_acquire_at(1, Duration::ZERO));
        let reserved = t1.join().unwrap().unwrap();
        let granted = t2.join().unwrap();

        if granted {
            // The grant came first, so the reservation waits for one token.
            assert_eq!(reserved, Duration::from_millis(500));
        } else {
            assert_eq!(reserved, Duration::ZERO);
        }
        assert_eq!(bucket.available_at(Duration::ZERO), 0);
    });
}
