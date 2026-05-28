# better-bucket — Benchmarks

Numbers for the lock-free acquire path, including the head-to-head against
`governor`.

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
| `try_acquire` — single thread (real clock) | 21.12 ns | **21.16 ns** | 21.21 ns |
| `try_acquire` — algorithm only (mock clock) | 5.27 ns | **5.31 ns** | 5.34 ns |
| `try_acquire` — contended, 2 threads | 10.74 ns | **10.94 ns** | 11.17 ns |
| `try_acquire` — contended, 4 threads | 9.59 ns | **10.16 ns** | 10.77 ns |
| `try_acquire` — contended, 8 threads | 16.14 ns | **17.38 ns** | 18.52 ns |
| `available` — refill after long idle | 4.02 ns | **4.03 ns** | 4.04 ns |

The dominant cost of a real `try_acquire` is the `Instant::now()` read. The
bucket's own accounting — the packed-word load, the fixed-point refill, and the
CAS — is **~5 ns** (`algorithm_only`, measured against a mock clock); the
remaining ~16 ns of the single-thread figure is the clock. Contended throughput
scales without a lock to serialize on.

> The contended figures are host-sensitive: the benchmark takes the slowest
> thread's time, so on a non-isolated machine a single descheduled thread inflates
> them (the 8-thread number especially swings run-to-run). Read them as evidence
> of lock-free scaling, not as precise per-operation constants.

## Head-to-head vs `governor`

Single-thread, allow path, same machine and same run. The fair comparison is on
the same clock; `governor`'s default clock (`quanta`, a TSC-calibrated read) is
faster than the `Instant` clock `clock-lib` provides, so its out-of-the-box
number is also shown.

| Limiter | algorithm | clock | low | median | high |
|---|---|---|---|---|---|
| `better-bucket` | token bucket | `Instant` | 22.72 ns | **22.84 ns** | 22.94 ns |
| `governor` | GCRA | `Instant` (monotonic) | 20.01 ns | **20.17 ns** | 20.37 ns |
| `governor` | GCRA | `quanta` (default) | 6.54 ns | **6.61 ns** | 6.67 ns |

### What this says, honestly

- **On the same `Instant` clock, `governor` is faster per call** — ~20.2 vs
  ~22.8 ns, roughly 10–13%. `better-bucket` does **not** beat it here, and the
  reason is the algorithm, not the implementation.
- **`governor` is GCRA; `better-bucket` is a token bucket.** GCRA stores one
  timestamp and compares it — about the least work a rate-limit decision can do.
  A token bucket tracks and *refills a token count*, clamps it to a capacity, and
  supports multi-token acquires, a burst ceiling, an `available()` snapshot, and
  a `retry_after` hint. Those are real semantics GCRA does not provide, and they
  cost a few nanoseconds of arithmetic per call. The token-bucket accounting
  itself (`algorithm_only`, ~5 ns) is as lean as that algorithm gets.
- **Out of the box, `governor` is faster still** (~6.6 ns), because its default
  `quanta` clock beats the `Instant` clock `better-bucket` reads through
  `clock-lib`. That part is a clock difference, not an algorithm one.

### Where "better" applies

`better-bucket` is the fastest, safest **token bucket** in its class — lock-free
and allocation-free where `leaky-bucket` uses a background task and hand-rolled
buckets use a `Mutex`, with a `loom`-proven no-over-grant contract and true token
semantics (counts, bursts, multi-token acquire, introspection). It is not a
GCRA, and it does not try to out-cycle one: for a pure allow/deny decision with
no token accounting, GCRA does less work and `governor` is the leaner choice.
Pick `better-bucket` when you want an actual token bucket; the per-call cost is
competitive and, end-to-end, bounded by the clock for both.

### The clock is the floor

`better-bucket`'s end-to-end latency is bounded by the monotonic clock, not its
own code. A faster monotonic source (e.g. a TSC reading) from `clock-lib` would
close most of the out-of-the-box gap with `governor`'s `quanta` clock; that is a
`clock-lib` improvement, noted for a future cross-crate change rather than worked
around here.

## Caveats

- Single runs on one machine; sub-nanosecond differences are within variance.
- WSL2 is not bare metal — absolute numbers on native Linux or other CPUs will
  differ. The shape (clock-dominated end-to-end, ~5 ns algorithm, scalable
  contention, a few ns behind GCRA on the same clock) is the takeaway.
- The contended benchmark reports the slowest thread's time after a barrier, so
  its absolute figures are host-sensitive; treat them as a scaling signal.
