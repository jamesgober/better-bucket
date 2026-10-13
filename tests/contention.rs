//! Real-clock contention and clock-skew regressions.
//!
//! 1.0.0 granted 36x–527x its quota under ordinary multi-threaded use with the
//! default clock (BB-COR-01): a thread holding a clock reading older than the
//! last committed one computed a ~49.7-day gap and refilled the bucket. Every
//! concurrency test then froze the clock, so none of them could see it. These
//! tests run real threads against the real `SystemClock` and assert the one
//! number that matters — total tokens granted against the ceiling the
//! configuration allows over the measured wall time — plus deterministic
//! reproductions of the stale and backward readings that caused it.
//!
//! Gated on `clock`: without it there is no `Bucket` to exercise.

#![cfg(feature = "clock")]
#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use better_bucket::{Bucket, BucketConfig, Decision};
use clock_lib::{Clock, ManualClock, Monotonic, Wall};

/// The most a bucket may grant over `elapsed`: its initial fill plus what the
/// rate accrues, plus one token of slack for the sub-nanosecond clock rounding.
fn ceiling(initial: u32, amount: u32, period: Duration, elapsed: Duration) -> u128 {
    u128::from(initial) + u128::from(amount) * elapsed.as_nanos() / period.as_nanos() + 1
}

/// Hammers one shared bucket from `threads` threads for `run_for`, returning
/// `(granted, elapsed)`. `elapsed` spans the bucket's whole life, from before
/// construction to after the last thread finished, so it bounds every grant.
fn hammer(config: BucketConfig, threads: usize, take: u32, run_for: Duration) -> (u64, Duration) {
    let start = Instant::now();
    let bucket = Arc::new(Bucket::from_config(config));
    let granted = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let bucket = Arc::clone(&bucket);
            let granted = Arc::clone(&granted);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let began = Instant::now();
                let mut local = 0_u64;
                while began.elapsed() < run_for {
                    if bucket.try_acquire(take) {
                        local += u64::from(take);
                    }
                }
                let _ = granted.fetch_add(local, Ordering::Relaxed);
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    (granted.load(Ordering::Relaxed), start.elapsed())
}

fn thread_counts() -> Vec<usize> {
    let cpus = thread::available_parallelism().map_or(2, |n| n.get());
    [2_usize, 4, 8, 16]
        .into_iter()
        .filter(|&n| n <= cpus.max(2))
        .collect()
}

#[test]
fn test_real_clock_contention_never_exceeds_the_rate_ceiling() {
    // 1000/s, capacity 1000: 1.0.0 granted ~89,000 here with just 2 threads.
    let config = BucketConfig::new(1_000, 1_000, Duration::from_secs(1), 1_000).unwrap();
    for threads in thread_counts() {
        let (granted, elapsed) = hammer(config, threads, 1, Duration::from_millis(400));
        let limit = ceiling(1_000, 1_000, Duration::from_secs(1), elapsed);
        assert!(
            u128::from(granted) <= limit,
            "{threads} threads: granted {granted} > ceiling {limit} over {elapsed:?}"
        );
        // And the bucket is not starved: at least the initial burst is handed
        // out. (A stricter share of the refill would depend on how much CPU a
        // loaded CI host gives the threads.)
        assert!(
            granted >= 1_000,
            "{threads} threads: granted only {granted} of {limit}"
        );
    }
}

#[test]
fn test_real_clock_contention_multi_token_and_odd_rate() {
    // A rate whose per-token time is not a whole number of nanoseconds, and
    // multi-token takes, under the same real-clock race.
    let config = BucketConfig::new(997, 6_001, Duration::from_secs(1), 0).unwrap();
    for threads in thread_counts() {
        let (granted, elapsed) = hammer(config, threads, 3, Duration::from_millis(300));
        let limit = ceiling(0, 6_001, Duration::from_secs(1), elapsed);
        assert!(
            u128::from(granted) <= limit,
            "{threads} threads: granted {granted} > ceiling {limit} over {elapsed:?}"
        );
    }
}

/// A clock that lets one designated thread read the time and then stall,
/// simulating preemption between the clock read and the state update.
struct StallClock {
    inner: Arc<ManualClock>,
    stalled: Arc<Barrier>,
    resume: Arc<Barrier>,
    armed: AtomicBool,
}

impl Clock for StallClock {
    fn now(&self) -> Monotonic {
        let reading = self.inner.now();
        if thread::current().name() == Some("victim") && self.armed.swap(false, Ordering::SeqCst) {
            self.stalled.wait(); // the victim holds `reading`...
            self.resume.wait(); // ...while the main thread moves time on
        }
        reading
    }

    fn wall(&self) -> Wall {
        self.inner.wall()
    }
}

#[test]
fn test_stale_reading_after_a_later_commit_grants_nothing_extra() {
    let inner = Arc::new(ManualClock::new());
    let stalled = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let clock = StallClock {
        inner: Arc::clone(&inner),
        stalled: Arc::clone(&stalled),
        resume: Arc::clone(&resume),
        armed: AtomicBool::new(true),
    };
    let config = BucketConfig::new(10, 10, Duration::from_secs(1), 10).unwrap();
    let bucket = Arc::new(Bucket::with_config_and_clock(config, clock));

    let shared = Arc::clone(&bucket);
    let victim = thread::Builder::new()
        .name("victim".into())
        .spawn(move || shared.try_acquire(1))
        .unwrap();

    stalled.wait(); // victim has read t = 0 and is "preempted"
    inner.advance(Duration::from_millis(1));
    assert!(bucket.try_acquire(10)); // drain at t = 1 ms
    resume.wait(); // victim resumes with its t = 0 reading
    let victim_granted = victim.join().unwrap();

    let mut extra = 0;
    while bucket.try_acquire(1) {
        extra += 1;
    }
    // 10 initial + 1 ms at 10/s (0.01 tokens): exactly 10 may be granted.
    assert!(
        !victim_granted,
        "a stale reading was granted from a drained bucket"
    );
    assert_eq!(extra, 0, "a stale reading refilled the bucket");
    assert_eq!(bucket.available(), 0);
}

/// A clock whose reading the test sets directly, including backwards — any
/// user `Clock` implementation may misbehave this way.
struct SettableClock {
    now: Mutex<Monotonic>,
    wall: Wall,
}

impl Clock for SettableClock {
    fn now(&self) -> Monotonic {
        *self.now.lock().unwrap()
    }

    fn wall(&self) -> Wall {
        self.wall
    }
}

#[test]
fn test_backward_clock_step_never_refills() {
    let manual = ManualClock::new();
    let t0 = manual.now();
    manual.advance(Duration::from_secs(5));
    let t5 = manual.now();
    let clock = Arc::new(SettableClock {
        now: Mutex::new(t0),
        wall: manual.wall(),
    });
    let config = BucketConfig::new(100, 100, Duration::from_secs(1), 100).unwrap();
    let bucket = Bucket::with_config_and_clock(config, Arc::clone(&clock));

    *clock.now.lock().unwrap() = t5;
    assert!(bucket.try_acquire(100)); // drain at t = 5 s
    assert_eq!(bucket.available(), 0);

    *clock.now.lock().unwrap() = t0; // the clock steps back 5 s
    assert_eq!(bucket.available(), 0, "a backward step refilled the bucket");
    assert!(!bucket.try_acquire(1));
    assert_eq!(bucket.time_until(1), Some(Duration::from_millis(5_010)));

    // Once time is back past the last commit, refill resumes normally.
    *clock.now.lock().unwrap() = t5;
    assert_eq!(bucket.available(), 0);
    manual.advance(Duration::from_millis(250));
    *clock.now.lock().unwrap() = manual.now();
    assert_eq!(bucket.available(), 25);
}

#[test]
fn test_reading_before_construction_counts_as_construction() {
    let early = SettableClock {
        now: Mutex::new(Monotonic::now()),
        wall: Wall::now(),
    };
    let before = early.now();
    thread::sleep(Duration::from_millis(2));
    *early.now.lock().unwrap() = Monotonic::now();
    let bucket = Bucket::with_config_and_clock(
        BucketConfig::new(5, 5, Duration::from_secs(1), 2).unwrap(),
        early,
    );
    // `before` predates the bucket: no refill, no panic, the initial 2 tokens.
    assert_eq!(bucket.available_at(before), 2);
    assert!(bucket.try_acquire_at(2, before));
    assert!(!bucket.try_acquire_at(1, before));
}

#[test]
fn test_concurrent_denials_at_a_frozen_instant_report_one_honest_wait() {
    let clock = Arc::new(ManualClock::new());
    let config = BucketConfig::new(8, 8, Duration::from_secs(1), 8).unwrap();
    let bucket = Arc::new(Bucket::with_config_and_clock(config, Arc::clone(&clock)));
    assert!(bucket.try_acquire(8));

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let bucket = Arc::clone(&bucket);
            thread::spawn(move || bucket.acquire(2))
        })
        .collect();
    for handle in handles {
        assert_eq!(
            handle.join().unwrap(),
            Decision::Denied {
                retry_after: Duration::from_millis(250)
            }
        );
    }
    clock.advance(Duration::from_millis(250));
    assert!(bucket.try_acquire(2), "the reported wait was not enough");
}

#[test]
fn test_reset_racing_acquires_under_an_advancing_clock_is_bounded() {
    // A manual clock advanced by its own thread while others acquire and one
    // resets: total grants stay within (resets + 1) bursts plus the refill.
    const RATE: u32 = 50;
    const RESETS: u64 = 200;
    const ADVANCES: u64 = 2_000;
    let clock = Arc::new(ManualClock::new());
    let bucket = Arc::new(Bucket::per_second(RATE).with_clock(Arc::clone(&clock)));
    let granted = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));

    let acquirers: Vec<_> = (0..4)
        .map(|_| {
            let bucket = Arc::clone(&bucket);
            let granted = Arc::clone(&granted);
            let done = Arc::clone(&done);
            thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if bucket.try_acquire(1) {
                        let _ = granted.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();
    let resetter = {
        let bucket = Arc::clone(&bucket);
        thread::spawn(move || {
            for _ in 0..RESETS {
                bucket.reset();
                thread::yield_now();
            }
        })
    };
    for _ in 0..ADVANCES {
        clock.advance(Duration::from_millis(1));
        thread::yield_now();
    }
    resetter.join().unwrap();
    done.store(true, Ordering::Relaxed);
    for handle in acquirers {
        handle.join().unwrap();
    }

    let accrued = u64::from(RATE) * ADVANCES / 1_000;
    let ceiling = u64::from(RATE) * (RESETS + 1) + accrued + 1;
    let granted = granted.load(Ordering::Relaxed);
    assert!(granted <= ceiling, "granted {granted} > ceiling {ceiling}");
    assert!(bucket.available() <= bucket.capacity());
}
