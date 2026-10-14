//! Criterion baselines for the lock-free acquire path.
//!
//! The single-thread `try_acquire` hot path (with the real clock, and isolated
//! from it), the caller-driven `BucketCore` path, the deny path that computes a
//! retry hint, contended acquire across several thread counts, and the refill
//! computation after a long idle gap. Numbers are recorded in
//! `docs/BENCHMARKS.md`; the comparison against `governor` is in
//! `benches/comparison.rs`.

use std::hint::black_box;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use better_bucket::{Bucket, BucketCore};
use clock_lib::ManualClock;
use criterion::{Criterion, criterion_group, criterion_main};

/// Single-thread `try_acquire` on a held bucket. Capacity is large and the
/// bucket is reset when it drains, so the measurement is dominated by the
/// granting CAS path rather than the deny path. Uses the real `SystemClock`, so
/// this figure includes one `Instant::now()` read per call.
fn bench_single_thread(c: &mut Criterion) {
    let bucket = Bucket::per_second(1_000_000);
    let _ = c.bench_function("try_acquire/single_thread", |b| {
        b.iter(|| {
            if !bucket.try_acquire(black_box(1)) {
                bucket.reset();
            }
        });
    });
}

/// The bucket's own work, isolated from the clock: a `ManualClock` (a cheap
/// atomic load, not a syscall) means this measures the packed-word load, the
/// fixed-point refill, and the CAS — without the `Instant::now()` cost that
/// dominates `single_thread`.
fn bench_algorithm_only(c: &mut Criterion) {
    let clock = Arc::new(ManualClock::new());
    let bucket = Bucket::per_second(1_000_000).with_clock(Arc::clone(&clock));
    let _ = c.bench_function("try_acquire/algorithm_only", |b| {
        b.iter(|| {
            if !bucket.try_acquire(black_box(1)) {
                bucket.reset();
            }
        });
    });
}

/// The caller-driven engine with a fixed reading: no clock at all, so this is
/// the accounting alone, as a `no_std` caller (or a keyed limiter amortising one
/// clock read across many buckets) sees it.
fn bench_core_acquire_at(c: &mut Criterion) {
    // 1M tokens/second, burst 1M, starting full.
    let config = Bucket::per_second(1_000_000).config();
    let bucket = BucketCore::new(config, Duration::ZERO);
    let now = Duration::from_millis(1);
    let _ = c.bench_function("core/acquire_at", |b| {
        b.iter(|| {
            if !bucket.try_acquire_at(black_box(1), black_box(now)) {
                bucket.reset_at(now);
            }
        });
    });
}

/// The deny path: an empty bucket that never refills within the benchmark,
/// so every call computes and returns a `retry_after` hint. This is the path an
/// abusive client exercises.
fn bench_deny_with_retry_hint(c: &mut Criterion) {
    let clock = Arc::new(ManualClock::new());
    let bucket = Bucket::per_second(10).with_clock(Arc::clone(&clock));
    let _ = bucket.try_acquire(10); // drain
    let _ = c.bench_function("acquire/denied_with_retry_after", |b| {
        b.iter(|| black_box(bucket.acquire(black_box(1))));
    });
}

/// `time_until` on a drained bucket: the non-consuming Retry-After query.
fn bench_time_until(c: &mut Criterion) {
    let clock = Arc::new(ManualClock::new());
    let bucket = Bucket::per_second(10).with_clock(Arc::clone(&clock));
    let _ = bucket.try_acquire(10); // drain
    let _ = c.bench_function("time_until/drained", |b| {
        b.iter(|| black_box(bucket.time_until(black_box(1))));
    });
}

/// Contended `try_acquire`: `threads` threads hammer one shared bucket. Reported
/// per-operation time includes the CAS retries that contention provokes.
fn bench_contended(c: &mut Criterion) {
    let mut group = c.benchmark_group("try_acquire/contended");
    for threads in [2_usize, 4, 8] {
        let _ = group.bench_function(format!("{threads}_threads"), |b| {
            b.iter_custom(|iters| {
                let bucket = Arc::new(Bucket::per_second(1_000_000));
                let per_thread = (iters / threads as u64).max(1);
                let barrier = Arc::new(Barrier::new(threads));

                let handles: Vec<_> = (0..threads)
                    .map(|_| {
                        let bucket = Arc::clone(&bucket);
                        let barrier = Arc::clone(&barrier);
                        thread::spawn(move || {
                            barrier.wait();
                            let start = Instant::now();
                            for _ in 0..per_thread {
                                let _ = bucket.try_acquire(black_box(1));
                            }
                            start.elapsed()
                        })
                    })
                    .collect();

                // Total time is the slowest thread's span.
                handles
                    .into_iter()
                    .map(|h| h.join().unwrap_or_default())
                    .max()
                    .unwrap_or_default()
            });
        });
    }
    group.finish();
}

/// Cost of bringing a long-idle bucket current — the refill computation after a
/// large elapsed gap, with the result clamped to capacity.
fn bench_refill_after_idle(c: &mut Criterion) {
    let clock = Arc::new(ManualClock::new());
    let bucket = Bucket::per_second(1_000).with_clock(Arc::clone(&clock));
    let _ = bucket.try_acquire(1_000); // drain
    clock.advance(Duration::from_secs(3_600)); // one idle hour
    let _ = c.bench_function("available/refill_after_idle", |b| {
        b.iter(|| black_box(bucket.available()));
    });
}

criterion_group!(
    benches,
    bench_single_thread,
    bench_algorithm_only,
    bench_core_acquire_at,
    bench_deny_with_retry_hint,
    bench_time_until,
    bench_contended,
    bench_refill_after_idle
);
criterion_main!(benches);
