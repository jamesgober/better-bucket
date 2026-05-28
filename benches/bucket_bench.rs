//! Criterion baselines for the lock-free acquire path.
//!
//! Three measurements, matching the roadmap's basic suite: the single-thread
//! `try_acquire` hot path, contended acquire across several thread counts, and
//! the refill computation after a long idle gap. Numbers are recorded in
//! `docs/BENCHMARKS.md`; the comparative benchmark against `governor` lands with
//! the optimization milestone.

use std::hint::black_box;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use better_bucket::Bucket;
use clock_lib::ManualClock;
use criterion::{Criterion, criterion_group, criterion_main};

/// Single-thread `try_acquire` on a held bucket. Capacity is large and the
/// bucket is reset when it drains, so the measurement is dominated by the
/// granting CAS path rather than the deny path.
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
    bench_contended,
    bench_refill_after_idle
);
criterion_main!(benches);
