# better-bucket — Benchmarks

Numbers for the lock-free acquire path, including the head-to-head against
`governor`.

> Baselines, not marketing. Taken on the machine below; treat them as
> order-of-magnitude and re-run locally before drawing conclusions on your own
> hardware. The comparison run needs the bench-only feature:
> `cargo bench --features comparison`.

## Method

- Harness: [`criterion`](https://crates.io/crates/criterion) 0.5, 100 samples
  per benchmark after a 2-second warm-up, 5-second measurement.
- Source: [`benches/bucket_bench.rs`](../benches/bucket_bench.rs) and
  [`benches/comparison.rs`](../benches/comparison.rs).
- Reported as the median of criterion's estimate, from two interleaved runs of
  each version (1.0.0, 1.1.0, 1.0.0, 1.1.0) on the same machine in the same
  session; where the two runs disagree, both are shown.
- `single_thread` uses the real `SystemClock`, so it includes one
  `Instant::now()` per call. `algorithm_only` uses a `ManualClock` (a cheap
  atomic load) to isolate the bucket's own work from the clock. `core/acquire_at`
  uses `BucketCore` with a fixed reading: no clock at all.

## Environment

| | |
|---|---|
| CPU | AMD Ryzen 9 9950X3D (16 cores / 32 threads) |
| OS | Windows 11 Pro (x86_64), not isolated: other work was running |
| Toolchain | rustc 1.95.0, release profile (`lto = "fat"`, `codegen-units = 1`) |

Windows' monotonic clock (QPC) is slower to read than Linux's vDSO `Instant`, so
the real-clock figures here are higher than the `1.0.0` table, which was taken on
Linux (WSL2) and read ~21 ns single-threaded. The clock-free figures are
comparable across the two.

## better-bucket 1.1.0 vs 1.0.0

| Benchmark | 1.0.0 | 1.1.0 |
|---|---|---|
| `try_acquire` — single thread (real clock) | 30.4 / 31.5 ns | 30.5 / 36.0 ns |
| `try_acquire` — algorithm only (mock clock) | 6.2 / 6.1 ns | 6.6 / 6.9 ns |
| `acquire_at` — `BucketCore`, no clock | — | **4.4 / 4.5 ns** |
| `acquire` — denied, computing `retry_after` | — | 11.0 / 13.0 ns |
| `time_until` — drained bucket | — | 14.2 / 11.5 ns |
| `try_acquire` — contended, 2 threads | 21.0 / 18.7 ns | 22.5 / 18.8 ns |
| `try_acquire` — contended, 4 threads | 11.7 / 17.3 ns | 15.2 / 12.4 ns |
| `try_acquire` — contended, 8 threads | 27.0 / 26.1 ns | 10.1 / 8.4 ns |
| `available` — refill after long idle | 4.7 / 5.6 ns | 5.6 / 3.7 ns |

What changed, honestly:

- **The accounting costs about half a nanosecond more** (`algorithm_only`,
  ~6.2 → ~6.7 ns). The clock reading is now converted to exact token units with
  a 64×64-bit multiply, where `1.0` divided it down to milliseconds and threw
  the remainder away. That is the price of exact token accounting and a clock
  that never wraps; the single-thread real-clock figure is within run-to-run
  noise of `1.0`, because the clock read dominates.
- **`BucketCore` with a caller-supplied reading is the fastest path** at
  ~4.5 ns: no clock read and no `Monotonic` arithmetic. A keyed limiter that
  reads the clock once per request and passes it to `acquire_at` gets close to
  this per bucket.
- **The contended numbers are not comparable with `1.0`'s.** `1.0` over-granted
  under contention, so its contended benchmark mostly measured granting CASes
  from a bucket that kept refilling itself. In `1.1` the bucket drains as it
  should and most contended calls are denials, which read the word and do not
  write it — so they scale better (8 threads: ~26 → ~9 ns), and the figure is now
  a mix of grants and denials.
- **The deny path** (computing `retry_after`, one 128-bit division) costs about
  twice a grant; `time_until` is the same computation without the decision.
- **Exact whole-token charging up to ~4.3 × 10^9 tokens/second** (one extra
  multiply in the clock conversion, taken only by buckets of 10^9 tokens/second
  and above) was measured A/B against the engine without it, interleaved in one
  session on a loaded host: `algorithm_only` 9.3 / 8.0 ns before vs 8.4 / 8.0 ns
  after, `core/acquire_at` 6.4 / 5.7 vs 6.4 / 6.4 ns, single-thread 35.2 / 34.2
  vs 35.4 / 34.0 ns — no difference beyond run-to-run noise. (Absolute figures
  in that session ran 20–30% above the table, because of the host load.)

> The contended figures are host-sensitive: the benchmark takes the slowest
> thread's time, so on a non-isolated machine a single descheduled thread inflates
> them (they swing run-to-run). Read them as evidence of lock-free scaling, not
> as precise per-operation constants.

## Head-to-head vs `governor`

Single-thread, allow path, same machine and same run. The fair comparison is on
the same clock; `governor`'s default clock (`quanta`, a TSC-calibrated read) is
faster than the `Instant` clock `clock-lib` provides, so its out-of-the-box
number is also shown.

| Limiter | algorithm | clock | run 1 | run 2 |
|---|---|---|---|---|
| `better-bucket` 1.1.0 | token bucket | `Instant` | 31.4 ns | 33.4 ns |
| `governor` 0.10 | GCRA | `Instant` (monotonic) | 34.5 ns | 30.8 ns |
| `governor` 0.10 | GCRA | `quanta` (default) | 7.7 ns | 7.6 ns |

### What this says

- **On the same `Instant` clock the two are within noise of each other** on this
  host (the 1.0.0 Linux run measured `governor` 10–13% ahead). Both are
  dominated by the clock read.
- **`governor` is GCRA; `better-bucket` is a token bucket.** Both now track a
  single instant, but a token bucket also answers multi-token acquires, a burst
  capacity, an `available()` snapshot, a `retry_after` hint, reservations,
  refunds, and partial grants. GCRA does a little less work per call.
- **Out of the box, `governor` is faster** (~7.6 ns), because its default
  `quanta` clock beats the `Instant` clock `better-bucket` reads through
  `clock-lib`. That is a clock difference, not an algorithm one; a caller with a
  faster time source can feed it to `BucketCore::acquire_at` (~4.5 ns) or
  `Bucket::acquire_at`.

The [`ratelimit`](https://crates.io/crates/ratelimit) crate is another lock-free
token bucket; it has not been benchmarked here.

### The clock is the floor

`better-bucket`'s end-to-end latency is bounded by the monotonic clock, not its
own code. A faster monotonic source (e.g. a TSC reading) from `clock-lib` would
close most of the out-of-the-box gap with `governor`'s `quanta` clock; that is a
`clock-lib` improvement, noted for a future cross-crate change rather than worked
around here.

## Caveats

- Two runs per version on one non-isolated machine; differences of a few
  hundred picoseconds, and contended differences of several nanoseconds, are
  within variance.
- The shape is the takeaway: clock-dominated end-to-end, ~5–7 ns of accounting,
  a clock-free path at ~4.5 ns, denials that do not contend, and parity with
  GCRA on the same clock.
