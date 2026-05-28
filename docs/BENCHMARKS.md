# better-bucket — Benchmarks

Numbers for the lock-free acquire path, including the head-to-head against
`governor`. Recorded at `0.6.0` (the optimization milestone).

> Honest baselines, not marketing. Taken on the machine below; treat as
> order-of-magnitude and re-run locally before drawing conclusions on your own
> hardware. The comparison run needs the bench-only feature:
> `cargo bench --features comparison`.

## Method

- Harness: [`criterion`](https://crates.io/crates/criterion) 0.5, 100 samples
  per benchmark after a 3-second warm-up.
- Source: [`benches/bucket_bench.rs`](../benches/bucket_bench.rs) and
  [`benches/comparison.rs`](../benches/comparison.rs).
- Reported as `[low  median  high]` from criterion's estimate.
- The `single_thread` benchmark uses the real `SystemClock`, so it includes one
  `Instant::now()` per call. `algorithm_only` uses a `ManualClock` (a cheap
  atomic load) to isolate the bucket's own work from the clock.

## Environment

| | |
|---|---|
| CPU | AMD Ryzen 9 9950X3D (16 cores / 32 threads) |
| OS | Linux 6.6 (WSL2, Ubuntu) |
| Toolchain | rustc 1.95.0, release profile (`lto = "fat"`, `codegen-units = 1`) |

## better-bucket

| Benchmark | low | median | high |
|---|---|---|---|
| `try_acquire` — single thread (real clock) | 23.86 ns | **24.14 ns** | 24.43 ns |
| `try_acquire` — algorithm only (mock clock) | 6.18 ns | **6.21 ns** | 6.25 ns |
| `try_acquire` — contended, 2 threads | 11.70 ns | **11.90 ns** | 12.22 ns |
| `try_acquire` — contended, 4 threads | 6.84 ns | **6.96 ns** | 7.10 ns |
| `try_acquire` — contended, 8 threads | 4.26 ns | **4.39 ns** | 4.53 ns |
| `available` — refill after long idle | 4.67 ns | **4.70 ns** | 4.73 ns |

The single-thread figure improved ~9% over the `0.5` baseline (26.5 → 24.1 ns)
after the `0.6` optimization: the per-acquire `u128` division was replaced by a
precomputed fixed-point multiply-and-shift, with an early return when no whole
millisecond has elapsed. The dominant cost is the `Instant::now()` read — the
bucket's own accounting (`algorithm_only`, with a cheap clock) is **~6 ns**, and
contended throughput scales with threads because the lock-free CAS has no lock
to serialize on.

## Head-to-head vs `governor`

Single-thread, allow path, same machine. The fair comparison is on the same
clock; `governor`'s default clock (`quanta`, a TSC-calibrated read) is faster
than the `Instant` clock `clock-lib` provides, so its out-of-the-box number is
also shown.

| Limiter | clock | low | median | high |
|---|---|---|---|---|
| `better-bucket` | `Instant` (clock-lib) | 23.84 ns | **24.02 ns** | 24.20 ns |
| `governor` | `Instant` (monotonic) | 23.09 ns | **23.17 ns** | 23.26 ns |
| `governor` | `quanta` (default) | 7.04 ns | **7.07 ns** | 7.11 ns |

### What this says, honestly

- **On the same `Instant` clock, the two are tied** — 24.0 vs 23.2 ns, within a
  nanosecond, both dominated by the ~20 ns clock read. `better-bucket` does not
  measurably beat `governor` here.
- **The bucket's algorithm is at least as lean.** `better-bucket` with a cheap
  clock (`algorithm_only`, ~6.2 ns) edges `governor` on its fast `quanta` clock
  (~7.07 ns). The token-bucket accounting is not the bottleneck.
- **Out of the box, `governor` is faster end-to-end** (7 ns vs 24 ns), entirely
  because its default `quanta` clock is faster than the `Instant` clock
  `better-bucket` reads through `clock-lib`. This is a clock difference, not an
  algorithm difference.

### Consequence

`better-bucket`'s end-to-end latency is bounded by the monotonic clock, not its
own code. Closing the out-of-the-box gap with `governor` requires a faster
monotonic source (e.g. a TSC-based reading) from `clock-lib`; that is a
`clock-lib` improvement, noted for a future cross-crate change rather than worked
around here. The bucket itself is already at the few-nanosecond floor.

## Caveats

- Single runs on one machine; sub-nanosecond differences are within variance.
- WSL2 is not bare metal — absolute numbers on native Linux or other CPUs will
  differ. The shape (clock-dominated end-to-end, ~6 ns algorithm, scalable
  contention, tied-with-`governor` on the same clock) is the takeaway.
- The contended benchmark reports per-thread time after a barrier (it excludes
  thread spawn/join), refined from the `0.5` methodology; the contended figures
  are not directly comparable across those two releases.
