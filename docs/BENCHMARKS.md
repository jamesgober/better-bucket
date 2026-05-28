# better-bucket — Benchmarks

Baseline numbers for the lock-free acquire path, recorded at the `0.5.0` feature
freeze. These establish the regression baseline the rest of the `0.x` series is
measured against; the head-to-head comparison with `governor` (and other
incumbents) lands with the optimization milestone in `0.6.0`.

> Numbers are honest baselines, not marketing. They were taken once on the
> machine below; treat them as order-of-magnitude, and re-run locally
> (`cargo bench --bench bucket_bench`) before drawing conclusions on your own
> hardware.

## Method

- Harness: [`criterion`](https://crates.io/crates/criterion) 0.5, 100 samples
  per benchmark after a 3-second warm-up.
- Source: [`benches/bucket_bench.rs`](../benches/bucket_bench.rs).
- Reported as `[low  median  high]` from criterion's estimate.

## Environment

| | |
|---|---|
| CPU | AMD Ryzen 9 9950X3D (16 cores / 32 threads) |
| OS | Linux 6.6 (WSL2, Ubuntu) |
| Toolchain | rustc 1.95.0, release profile (`lto = "fat"`, `codegen-units = 1`) |

## Results

| Benchmark | low | median | high |
|---|---|---|---|
| `try_acquire` — single thread | 26.24 ns | **26.54 ns** | 26.81 ns |
| `try_acquire` — contended, 2 threads | 16.56 ns | **16.82 ns** | 17.16 ns |
| `try_acquire` — contended, 4 threads | 10.06 ns | **10.37 ns** | 10.73 ns |
| `try_acquire` — contended, 8 threads | 9.19 ns | **9.59 ns** | 10.13 ns |
| `available` — refill after long idle | 4.17 ns | **4.19 ns** | 4.22 ns |

## Reading the numbers

- **The monotonic clock read dominates the single-thread figure.** The
  single-thread benchmark uses the real `SystemClock`, so every `try_acquire`
  pays for one `Instant::now()` — on the order of ~20 ns on this platform. The
  `refill_after_idle` benchmark uses a `ManualClock` (a cheap atomic load), which
  isolates the bucket's own work — the packed-word load, the saturating refill
  math, and the CAS — at **~4 ns**. In other words, the accounting itself is a
  handful of nanoseconds; the dominant cost in production is whatever the
  injected clock costs to read.
- **Contended per-operation time falls as threads are added**, because the
  contended benchmark reports per-operation wall time across a fixed total of
  acquires: more threads complete more work per wall-second. The lock-free CAS
  loop scales without a lock to serialize on; there is no single-writer
  bottleneck to collapse under load.
- **Refill after a long idle is constant-time** — it is one subtraction, one
  multiply/divide, and a clamp, regardless of how much time elapsed.

## Caveats

- A single run on one machine. Variance between runs is a few percent.
- WSL2 is not bare metal; absolute numbers on native Linux or other CPUs will
  differ. The *shape* (clock-read-dominated single thread, scalable contention,
  constant refill) is what matters here.
- The comparative benchmark against `governor` — the number that has to justify
  the crate's name — is deferred to `0.6.0`, where it will be committed
  alongside method and any case that does not win.
