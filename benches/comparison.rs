//! Head-to-head comparison against `governor`.
//!
//! Built only with `--features comparison`:
//! `cargo bench --features comparison --bench comparison`.
//!
//! The honest comparison is on the *same* clock. `better-bucket` reads time
//! through `clock-lib` (an `Instant`-based monotonic clock); `governor`'s
//! default clock is `quanta` (a faster, TSC-calibrated read). To compare the
//! algorithms rather than the clocks, the primary pairing runs both on a
//! monotonic (`Instant`) clock. `governor`'s default-`quanta` configuration is
//! also measured, to show the out-of-the-box reality.
//!
//! All benchmarks measure the single-thread *allow* path: a bucket/limiter
//! large enough that requests are granted, so the cost is clock read + the
//! grant update, not the deny path.

use std::hint::black_box;
use std::num::NonZeroU32;

use better_bucket::Bucket;
use criterion::{Criterion, criterion_group, criterion_main};
use governor::clock::MonotonicClock;
use governor::{Quota, RateLimiter};

const RATE: u32 = 4_000_000;

fn bench_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("acquire_one");

    // better-bucket — clock-lib SystemClock (Instant-based).
    let bucket = Bucket::per_second(RATE);
    let _ = group.bench_function("better_bucket", |b| {
        b.iter(|| {
            if !bucket.try_acquire(black_box(1)) {
                bucket.reset();
            }
        });
    });

    // governor — monotonic (Instant) clock: same time source as better-bucket,
    // so this isolates the algorithm. A u32::MAX burst never drains in a bench.
    let quota = Quota::per_second(NonZeroU32::new(u32::MAX).unwrap());
    let governor_monotonic = RateLimiter::direct_with_clock(quota, MonotonicClock);
    let _ = group.bench_function("governor_monotonic", |b| {
        b.iter(|| black_box(governor_monotonic.check().is_ok()));
    });

    // governor — its default `quanta` clock (the out-of-the-box configuration).
    let governor_default = RateLimiter::direct(quota);
    let _ = group.bench_function("governor_quanta", |b| {
        b.iter(|| black_box(governor_default.check().is_ok()));
    });

    group.finish();
}

criterion_group!(benches, bench_comparison);
criterion_main!(benches);
