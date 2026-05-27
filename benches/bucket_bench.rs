//! Criterion benchmark harness for `better-bucket`.
//!
//! `0.1.0` is the scaffold: there is no acquire path to measure yet, so this
//! suite establishes a baseline for the one primitive the lock-free core is
//! built on — a `compare_exchange_weak` update of a packed atomic word. The
//! real `try_acquire`, contended-acquire, and refill-after-idle benchmarks land
//! alongside the core implementation in `0.3.0`.

use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{Criterion, criterion_group, criterion_main};

/// Mirror of the future acquire mechanic: read the packed word, derive the next
/// value, and commit it with a CAS, retrying on contention. Measuring it in
/// isolation gives an honest floor for what the acquire path can cost.
#[inline]
fn packed_cas_update(state: &AtomicU64, delta: u64) {
    let mut current = state.load(Ordering::Relaxed);
    loop {
        let next = current.wrapping_sub(delta);
        match state.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

fn bench_packed_cas(c: &mut Criterion) {
    let state = AtomicU64::new(u64::MAX);
    let _ = c.bench_function("packed_cas_update/uncontended", |b| {
        b.iter(|| packed_cas_update(black_box(&state), black_box(1)));
    });
}

criterion_group!(benches, bench_packed_cas);
criterion_main!(benches);
