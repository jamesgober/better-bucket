<h1 align="center">
    <img width="90px" height="auto" src="https://raw.githubusercontent.com/jamesgober/jamesgober/main/media/icons/hexagon-3.svg" alt="Triple Hexagon">
    <br>
    <b>CHANGELOG</b>
</h1>
<p>
  All notable changes to <code>better-bucket</code> will be documented in this file. The format is based on <a href="https://keepachangelog.com/en/1.1.0/">Keep a Changelog</a>,
  and this project adheres to <a href="https://semver.org/spec/v2.0.0.html/">Semantic Versioning</a>.
</p>

---

## [Unreleased]

---

## [1.1.0] — 2026-10-13

A correctness release with additive features. **Upgrade from 1.0.0 is strongly
recommended**: 1.0.0 over-granted by one to two orders of magnitude under
ordinary multi-threaded use with the default clock. Every 1.0 call site compiles
and keeps its meaning; the fix is inside the existing methods.

### Security

- **Rate-limit bypass under concurrency (BB-SEC-01).** With the default
  `SystemClock` and two or more threads, 1.0.0 admitted 36x–527x its quota
  (measured: 2 threads granted ~89,000 tokens against a ~2,500 ceiling). Any
  client able to send parallel requests through a 1.0.0 bucket could bypass the
  limit. Fixed by the representation change below; the `rate-net` limiter
  inherits the fix with no code change.
- `cargo audit` / `cargo deny` are clean again: the dev-only advisories
  RUSTSEC-2026-0204 (`crossbeam-epoch` via `criterion`) and RUSTSEC-2026-0190
  (`anyhow` via `proptest`) are resolved by a lockfile refresh, and CI now
  resolves a fresh lockfile before auditing.
- `#![forbid(unsafe_code)]` locks in the crate's zero-`unsafe` status.

### Fixed

- **Over-grant from a stale clock reading (BB-COR-01).** A thread read the clock
  before its CAS loop; if another thread committed a later timestamp first, the
  stale reading's `wrapping_sub` produced a ~49.7-day "elapsed" time, refilled
  the bucket to full, and wrote the older timestamp back. The state is now the
  single instant the bucket would have been empty (the token-bucket reading of
  GCRA): a grant only moves it forward, and a stale or backward reading only
  sees fewer tokens. No skew window, no stall for long-idle buckets.
- **Backward custom clocks refilled the bucket (BB-COR-04).** Same root cause,
  same fix: a `Clock` that steps back now sees an emptier bucket until time
  catches up.
- **`available()` over-reported** under the same race; it now uses the same
  arithmetic as a grant.
- **Slow rates truncated or never refilled (BB-COR-02).** The Q22 millitoken
  rate made 1/week refill 15.6% slow, 1/30 days 62% slow, and anything slower
  than ~1 per 48.5 days never refill. Token amounts are now exact binary
  fractions (for every rate up to ~4.3 × 10^9 tokens/second) and the clock
  conversion is a 64-bit fixed-point rate whose error stays under one unit for
  the bucket's whole life: one per day, week, 60 days, or century refills on
  time.
- **Frequent grants discarded the refill remainder.** Each grant wrote "now" as
  the last refill and dropped the sub-millitoken fraction, so a 1-token/hour
  bucket drained once a second never refilled. The refill is now a function of
  absolute time and is never discarded.
- **Silent capacity clamp (BB-COR-03).** Capacities above ~4.29 M tokens were
  clamped, so `config().capacity()` and `capacity()` disagreed and an
  in-capacity request was denied forever. Capacity is now exact for every
  `u32`, and so is every grant for every `u32` amount per second; only rates
  above ~4.3 × 10^9 tokens/second (which need a sub-second period) round each
  grant up to a multi-token unit.
- **Idle gaps of 2^32 ms or more under-refilled (BB-COR-05).** Time is now
  nanoseconds that never wrap (at least ~438 years of range below 10^9
  tokens/second, ~100 years above), so an idle gap of any length refills as
  configured. Past the end of that range the clock is frozen and fails closed:
  a charge that would land beyond it is refused, so even a caller-supplied
  `Duration::MAX` reading can spend at most the tokens on hand.
- `retry_after` no longer over-states waits for rates whose per-millisecond
  refill was not an integer: 3 tokens per 7 ms now reports 7 ms, not 8 ms. It
  remains rounded up to the millisecond and honest.

### Added

- **`BucketCore`** — the engine with caller-supplied time
  (`Duration` since any epoch): `new`, `acquire_at`, `try_acquire_at`,
  `acquire_up_to_at`, `available_at`, `time_until_at`, `reserve_at`, `refund`,
  `reset_at`, `capacity`, `config`. Needs only `core` and a 64-bit atomic — the
  real `no_std` path that 1.0's docs described but did not ship. Lock-free on
  native 64-bit atomics.
- **Caller-supplied time on `Bucket`**: `acquire_at`, `try_acquire_at`,
  `available_at`, `time_until_at`, `acquire_up_to_at`, `reserve_at`, `reset_at`,
  taking a `Monotonic` reading (one clock read for several buckets), and
  `clock()` to get at the bucket's clock.
- **`Bucket::with_config_and_clock(config, clock)`** and
  **`BucketBuilder::build_with_clock(clock)`** — build once, read the clock once,
  instead of `from_config(..).with_clock(..)` building twice (BB-PERF-03).
- **`time_until(n)`** — the wait until `n` tokens could be acquired, without
  taking any (`Some(ZERO)` now, `Some(wait)` as an upper bound, or `None` for
  never).
- **`refund(n)`** — return tokens for cancelled or failed work, capped at the
  capacity.
- **`reserve(n, max_wait)`** — take tokens ahead of their accrual and get the
  wait; later callers queue behind it, so large requests cannot be starved by
  small ones (BB-COR-06).
- **`acquire_up_to(n)`** — partial grant: take what is available, up to `n`.
- **`TokenBucket::time_until` and `TokenBucket::reset`**, with provided bodies so
  existing implementations keep compiling; `Bucket` overrides both. The provided
  `time_until` keeps `None` meaning "never" (`n` above the capacity) and answers
  `Some(Duration::MAX)` — "not now, no better bound known" — when it cannot see
  a rate; the provided `reset` does nothing.
- **`BucketBuilder::build_config()`**, and `const fn` for `BucketBuilder::new`,
  `capacity`, `refill`, `initial`, `Bucket::builder`, and `BucketConfig::new`
  (compile-time-validated configurations).
- **`Hash`** for `BucketConfig`, `Decision`, and `BucketError`.
- **`serde` feature** (off by default): `Serialize` and a validating
  `Deserialize` for `BucketConfig` that rejects unknown fields.
- **`metrics` feature** (off by default): `BucketMetrics` with allowed / denied
  / granted-token counters, via `metrics()` on `Bucket` and `BucketCore`. The
  counters do not exist when the feature is off.
- **`forge` feature**: the `error_forge::ForgeError` impl, now optional for a
  core-only build. Implied by `clock`, so every 1.0 feature combination keeps it
  (BB-CQ-02).
- **`portable-atomic` feature** (off by default): `BucketCore` on targets
  without native 64-bit atomics, such as `thumbv7em-none-eabihf`. There
  portable-atomic's default fallback is a global spinlock: not lock-free, and
  not interrupt-safe unless the final binary enables portable-atomic's
  `unsafe-assume-single-core` or `critical-section` feature.
- `examples/caller_driven.rs` — the `no_std`-style core driven by a tick counter.
- Tests: a real-clock multi-thread grant-ceiling test (2–16 threads) and a
  real-clock matrix of eight configurations × four operation mixes; a
  deterministic stalled-reader test and a backward-clock test; `loom` models of
  the real `Bucket` and `BucketCore` with the clock itself modelled as a loom
  atomic (an advancing clock, stale readers, caller time from a shared atomic,
  `reset`, `refund`, and `reserve` races; every model fails against a mutant
  that replaces the CAS with a plain store); a `BucketCore` property suite over
  `u32`-sized capacities, rates from one per month to ~4 × 10^18 per second, and
  backward time (every window bounded, exact conservation, exact grant
  boundary, honest hints, reservations); clock-horizon saturation tests at
  `Duration::MAX` and 450–500 years; `serde` and `metrics` suites; and README /
  `docs/API.md` examples run as doctests.

### Changed

- `BucketConfig`, `BucketBuilder` (except `build` / `build_with_clock`),
  `Decision`, and `BucketError` are available without the `clock` feature;
  `BucketError` implements `core::error::Error` in every build.
- `Bucket<SystemClock>` is now exactly one 64-byte cache line (it was two)
  without the `metrics` feature (BB-PERF-02).
- `error-forge` requirement raised to `1.0.2`, which drops its unused
  `thiserror` / `syn` build dependencies from the default tree.
- CI: an aarch64 Linux runner, all-runtime-feature and per-feature test runs, a
  `--all-features` clippy job, a real `no_std` build for `thumbv7em-none-eabihf`
  (bare and with `portable-atomic`), a 32-bit Linux test job, a Miri job (also
  over the `portable-atomic` path), an AddressSanitizer job, a
  `cargo semver-checks` job, the `loom` job in release mode, and a fresh
  lockfile before `cargo audit`.
- `rustfmt.toml` drops the nightly-only `imports_granularity` and
  `group_imports` options that stable rustfmt ignored with a warning.
- Documentation corrected to match the code: the "proven by loom" and "never
  over-grants" claims now describe what is actually checked (the 1.0 loom model
  froze the clock); the `no_std` "caller drives time" text now describes a real
  API; `Duration::MAX` is documented as "never" for both the over-capacity and
  the beyond-the-clock-range case; the stale `0.2` / `0.3` / `Mutex` comments are
  gone; the outdated claim that `leaky-bucket` uses a background task, the
  unbenchmarked "fastest, safest in its class" claim, and the marketing tone
  are removed; the CI-platform claim matches the matrix; emoji are removed from
  the README, as REPS DOCUMENTATION requires.

### Notes

- **No breaking changes.** `cargo semver-checks` passes against 1.0.0; every
  1.0 signature is unchanged and every new item is additive.
- **Behaviour you may notice** (all of it bug or precision fixes):
  - `capacity()` returns the configured value above ~4.29 M.
  - Some `retry_after` values are a millisecond shorter (more exact).
  - A bucket idle for 49.7+ days refills fully.
  - Refill has nanosecond rather than millisecond resolution, so fast buckets
    accrue between whole milliseconds: `per_second(10_000)` has 5 tokens back
    500 µs after draining (1.0: 0), and a bucket polled every few microseconds
    reaches its configured rate (1.0 under-granted, up to 10x).
  - Rates above ~4.3 × 10^9 tokens/second (only reachable with a sub-second
    period) charge each grant in whole multi-token units — `acquire(1)` on
    `per_duration(u32::MAX, Duration::from_millis(1))` costs one 1,024-token
    unit. 1.0 clamped such buckets to ~4.29 M capacity instead.
  - A stale or backward clock reading is denied instead of refilling the
    bucket.
- **MSRV unchanged at Rust 1.85.**
- Consumers on 1.0.0 should upgrade; `rate-net` should require `>= 1.1.0`.

---

## [1.0.0] - 2026-05-28

Stable. The public API is frozen under strict SemVer; see
[`docs/STABILITY.md`](docs/STABILITY.md) for the committed surface and the
contract. Pin `better-bucket = "1"` and expect `1.x` to stay backwards
compatible.

### Added

- `docs/STABILITY.md` — the frozen `1.x` surface, SemVer commitments, behavioural
  contracts, representation limits, and what is explicitly not promised.
- A high-contention, multi-token exact-conservation test: 16 threads against a
  no-refill bucket whose capacity is not a multiple of the take size, asserting
  every token is either granted or still available — none lost, duplicated, or
  granted beyond capacity.
- `examples/admission_control.rs` — the canonical use case: rate-limit requests
  and answer each denial with a `Retry-After` derived from the `Decision`.

### Changed

- The contended CAS retry now emits a `core::hint::spin_loop()` hint, easing
  cache-line pressure under contention; the uncontended path (which never
  retries) is unchanged.
- `#[inline]` added to the public `acquire` / `try_acquire` / `available` methods
  and the `TokenBucket` trait impl, so the hot path inlines into consumer code.
  The bucket's own accounting now measures ~5 ns.
- `docs/BENCHMARKS.md` refreshed with the final `1.0` numbers and an honest
  `governor` comparison: `governor` is GCRA (one timestamp) and is a little
  faster per call than a token bucket on the same clock; `better-bucket` is the
  fastest, safest token bucket in its class, with semantics GCRA does not
  provide. The clock dominates end-to-end for both.
- Documentation polished for release: pre-1.0 status notes removed, every public
  item in `docs/API.md` carries multiple examples, and inaccurate claims (the
  old "no_std-capable" framing, the stale `retry with bounded backoff` note, and
  a `Cargo.toml` comment that still described a `Mutex`) corrected.
- Crate keyword `lock-free` replaced with `ratelimit` — a quality attribute
  swapped for a search term consumers actually use to find a rate limiter.

### Notes

- **No breaking changes.** Every `0.9.5` call site compiles and behaves
  identically.
- **MSRV frozen at Rust 1.85.**

---

## [0.9.5] - 2026-05-28

Release candidate. Documentation polish only — no code or API change. The
`0.9.1`–`0.9.4` bug-fix slots went unused; none were needed.

### Added

- `docs/DESIGN.md` — the architecture and design-decision reference: the packed
  `AtomicU64` layout and why tokens are stored in millitokens, the CAS acquire
  loop and its `Relaxed` ordering, the division-free `Q22` fixed-point refill,
  the wrapping millisecond clock, overflow safety, how the no-over-grant contract
  is defended, the clock as the performance floor, and the recorded design
  decisions (token-bucket-only, plain `u32`, no keyed store, zero `unsafe`, no
  SIMD).
- A `Documentation` section in the README linking the API reference, design doc,
  benchmarks, and changelog.

### Changed

- README testing commands aligned with CI (default features rather than
  `--all-features`, plus the `no_std`, doc, and security-gate commands), with a
  note that the `comparison` feature is benchmark-only.

---

## [0.9.0] - 2026-05-28

Beta. Broader testing and final benchmark numbers; no bug fixes were needed and
no API changed.

### Added

- A property test, `retry_after_is_an_honest_lower_bound`: after a denial,
  waiting exactly the reported `retry_after` always makes the same request
  succeed. This exercises the fixed-point `time_for` ceiling against the
  fixed-point refill floor across the input space, confirming the two agree so
  the retry hint never under-promises.

### Notes

- Benchmark numbers re-confirmed after the `0.7` wrap change and recorded as the
  stable beta baseline in `docs/BENCHMARKS.md`; no regression against `0.6`.
- No outstanding bugs. `0.9.1`–`0.9.4` are reserved for bug fixes should any
  surface; `0.9.5`+ is the release-candidate band (critical fixes and doc polish
  only) ahead of `1.0.0`.

---

## [0.8.0] - 2026-05-27

Alpha. The public surface is validated against the first-consumer pattern; no
API friction was found, so nothing changed.

### Added

- `tests/consumer_pattern.rs` — a first-consumer shake-out that exercises the
  surface the way a downstream limiter (e.g. `rate-net`) will: coding against
  the `TokenBucket` trait rather than the concrete type, holding buckets with
  different clocks behind `&dyn TokenBucket`, per-key buckets sharing one
  injected clock, mapping `Decision` to an allow/deny with a retry hint, and the
  config-then-inject-clock construction path. All four patterns read naturally
  and pass.

### Fixed

- The allocation audit (`tests/alloc.rs`) now counts allocations **per thread**
  instead of process-wide, and warms every operation it measures. The global
  counter could attribute incidental allocations made by the test harness or
  runtime on other threads to the acquire path, which produced a spurious
  failure on macOS/stable. The acquire path remains allocation-free; the test is
  now robust across platforms.

### Notes

- **No API change.** The frozen surface is consumable as-is; the trait plus
  clock injection plus `Decision` cover the keyed-limiter use case without
  friction. The keyed store itself remains the consumer's concern, not this
  crate's.
- Real integration against `rate-net` happens in that crate when it is built;
  this release confirms the surface is ready for it.

---

## [0.7.0] - 2026-05-27

Hardening and API freeze sign-off. No signature changes; the public surface is
frozen until 1.0.

### Added

- `tests/hardening.rs` — 13 adversarial and edge cases run by the full CI
  matrix (Linux/macOS/Windows × stable + MSRV): requests at `u32::MAX`,
  capacity/refill at `u32::MAX`, extreme and near-zero rates, the capacity
  boundary matrix (`0` / `1` / `n-1` / `n` / `n+1`), exact-empty and
  exact-full, zero-time-delta, a clock that never advances, enormous single
  time advances, and concurrent acquire-while-reset. Each asserts the safety
  contract: no panic, no wrap or overflow, no over-grant, tokens always within
  `[0, capacity]`.

### Changed

- **The millisecond time field now wraps instead of saturating.** Elapsed time
  is computed with `wrapping_sub`, which is correct for any gap shorter than the
  ~49.7-day wrap window — i.e. for any bucket used at least once in that window,
  which is every real limiter. This removes the long-uptime refill **stall** of
  prior releases (where the counter pinned at `u32::MAX` and refill stopped, and
  `reset` could not recover it because the time anchor is fixed at construction).
  A bucket left fully idle for longer than ~49.7 days may under-refill once on
  its next use — a safe, self-correcting outcome. Internal change, no API impact;
  all safety invariants continue to hold (proven by `tests/hardening.rs` and
  `loom`).
- `Bucket::reset` documentation clarified: it discards debt for a fresh burst;
  it is no longer described as a long-uptime workaround, which is unnecessary now
  that the time field wraps safely.

### Notes

- **Public API frozen** as of this release (recorded in full in the project
  roadmap): `Bucket`, `BucketBuilder`, `BucketConfig`, `Decision`,
  `BucketError`, `TokenBucket`, `VERSION`. Only additive, non-breaking changes
  through to 1.0.
- Cross-platform atomic behavior is verified by the CI matrix running the unit,
  stress, and hardening suites on Linux, macOS, and Windows on both stable and
  MSRV.

---

## [0.6.0] - 2026-05-27

Optimization. The acquire path is faster and division-free; the comparative
benchmark against `governor` is recorded. No API changes.

### Changed

- The per-acquire refill no longer divides. The refill rate is precomputed once
  at construction as a `Q22` fixed-point millitokens-per-millisecond multiplier,
  so the hot path is a multiply and a shift; an early return skips it entirely
  when no whole millisecond has elapsed since the last refill (the common case
  under bursty load). Single-thread `try_acquire` is ~9% faster than `0.5`
  (26.5 → 24.1 ns); the bucket's own accounting measures ~6 ns.
- CI tests the default feature set (`std` + `clock`) and the `no_std` build
  explicitly rather than `--all-features`, and `docs.rs` builds `std` + `clock`,
  so the benchmark-only `governor` dependency never enters CI, MSRV, or docs
  builds.

### Added

- `benches/comparison.rs` and a benchmark-only `comparison` feature (pulls
  `governor`): the head-to-head measured on the same monotonic clock, plus
  `governor`'s default `quanta`-clock configuration.
- An `algorithm_only` benchmark that isolates the bucket's work from the clock
  read using a `ManualClock`.
- `docs/BENCHMARKS.md` rewritten with the `0.6` numbers and an honest `governor`
  comparison: tied on the same `Instant` clock, the bucket's algorithm at least
  as lean as `governor`'s, and `governor` faster out-of-the-box purely because
  its default `quanta` clock beats `clock-lib`'s `Instant` read.

### Notes

- **No SIMD / no batched acquire.** A single-bucket acquire is one CAS on one
  word — no lane-parallelism to exploit, and no consumer needs batched
  multi-bucket acquire. Declined per the "evaluate, don't force" rule.
- The end-to-end latency is bounded by the monotonic clock, not the bucket.
  Matching `governor` out-of-the-box would need a faster monotonic source from
  `clock-lib`; recorded as a future cross-crate improvement.

---

## [0.5.0] - 2026-05-27

Feature complete. The public API is **frozen** until 1.0 — only additive,
non-breaking changes from here.

### Added

- `BucketBuilder` and `Bucket::builder()` — the Tier-2 fluent configuration
  path: `.capacity(..)`, `.refill(amount, period)`, `.initial(..)`, `.build()`.
  `build` validates through `BucketConfig::new`, so an unworkable combination
  is rejected; `initial` defaults to the capacity (start full). A custom clock
  is injected by chaining `Bucket::with_clock` onto the built bucket.
- `examples/` — `per_second` (Tier-1 limiter), `burst` (spike absorption with a
  `ManualClock`), `deterministic_test` (sleep-free testing pattern), and
  `builder` (Tier-2 configuration). Each declares `required-features = ["clock"]`.
- `benches/bucket_bench.rs` rewritten to measure the real lock-free `Bucket`:
  single-thread `try_acquire`, contended acquire across 2/4/8 threads, and the
  refill computation after a long idle gap.
- `docs/BENCHMARKS.md` — recorded baseline numbers with methodology and machine
  details (the comparative benchmark against `governor` lands in `0.6.0`).
- A refill-after-long-idle test asserting a multi-year gap saturates to
  capacity without wrapping or overflowing.

### Changed

- Documented the design decision that **token bucket is this crate's sole
  algorithm**; leaky-bucket and sliding-window limiting belong in `rate-net`.
- The benchmark target now requires the `clock` feature; a bare `no_std` build
  skips it (and the examples) rather than failing to compile.

---

## [0.3.0] - 2026-05-27

The lock-free core. The mutex-backed `0.2` internals are replaced by a single
atomic word and a CAS loop; the public surface is unchanged.

### Added

- `Bucket::reset()` — refills to full and re-anchors the internal clock.
  Useful to grant a fresh burst, and to keep refill alive on processes that
  run past the ~49.7-day millisecond-counter saturation window.
- `loom` model check (`tests/loom_acquire.rs`) of the real acquire path,
  proving the CAS grants exactly the available tokens — no over-grant, no
  lost token — across every interleaving.
- Multi-thread stress test (eight threads contending one bucket) asserting
  total grants never exceed the available tokens.
- Allocation audit (`tests/alloc.rs`) under a counting global allocator,
  asserting the acquire path performs zero allocations.

### Changed

- The acquire path is now **lock-free and allocation-free**: all mutable
  state lives in one `AtomicU64` packing tokens (millitokens, upper 32 bits)
  and milliseconds-since-creation (lower 32 bits), and `try_acquire` is a
  single `compare_exchange_weak` loop with lazy refill from the injected
  monotonic clock. The bucket is `#[repr(align(64))]` to prevent false
  sharing between independent buckets. Refill math is computed from the
  refill period in nanoseconds, so sub-millisecond periods stay correct
  despite the millisecond time tick.
- The packed representation introduces two documented limits (previously the
  mutex impl had neither): capacity is effectively capped at ~4.29 million
  tokens (`u32::MAX` millitokens; larger values clamp), and `retry_after` is
  reported at millisecond resolution.
- `loom` moved from a `cfg(loom)` dev-dependency to a `cfg(loom)` dependency,
  because the library's atomics now switch to `loom::sync::atomic` under that
  cfg and the lib crate itself must link it.

---

## [0.2.0] - 2026-05-27

The foundation release. The public surface is locked on a simple, correct,
single-threaded implementation; the lock-free core in `0.3.0` replaces the
internals without changing any of these signatures.

### Added

- `Bucket<C: Clock = SystemClock>` — the single token bucket. Tier-1
  constructors `per_second` and `per_duration` (infallible; a zero rate
  yields a deny-all bucket), `from_config` for full control, and the
  `with_clock` clock-injection seam.
- Acquire surface: `try_acquire(n) -> bool` (the one-line convenience),
  `acquire(n) -> Decision` (the full outcome with a retry hint),
  `available() -> u32`, `capacity() -> u32`, and `config()`. Zero-token
  requests always succeed; requests above capacity can never succeed.
- `BucketConfig` — validated parameters (capacity, refill amount, refill
  period, initial fill) via `BucketConfig::new`, which rejects zero
  capacity / amount / period and clamps `initial` to `capacity`. `const`
  getters for each field.
- `Decision` — the `#[non_exhaustive]` acquire outcome (`Allowed` /
  `Denied { retry_after }`) with `is_allowed`, `is_denied`, and
  `retry_after` helpers. `Denied` reports the minimum wait until the
  request would succeed (`Duration::MAX` when it never can).
- `BucketError` — the `#[non_exhaustive]` construction error
  (`ZeroCapacity`, `ZeroRefillAmount`, `ZeroRefillPeriod`), implementing
  `std::error::Error`, `Display`, and `error_forge::ForgeError`.
- `TokenBucket` trait — the object-safe surface a consumer (e.g.
  `rate-net`) codes against, implemented for every `Bucket<C>`.
- Lazy, overflow-safe refill computed from the monotonic clock at
  millitoken resolution; saturating arithmetic throughout.
- `tests/invariants.rs` — a `proptest` suite encoding the safety
  contract: tokens stay within `[0, capacity]`, and grants never exceed
  the initial fill plus accrued refill.
- `ManualClock`-driven unit tests for refill correctness, edge cases
  (exact-empty, exact-full, `n=0`, `n>capacity`), and `Send + Sync`.

### Changed

- The `clock` feature now implies `std` and gates the `Bucket` surface:
  `clock-lib`'s `Clock` trait is std-gated, and the simple implementation
  uses a `Mutex`. A bare `no_std` build exposes only `VERSION` until the
  caller-driven core arrives in `0.3.0`.
- Added the `error-forge` dependency (optional, enabled by `clock`) for
  the domain error type, per the portfolio error convention.
- `README.md` and `docs/API.md` document the full surface with examples;
  the Tier-2 examples use `BucketConfig` + `from_config` (the `builder`
  remains a planned `0.5` convenience).

---

## [0.1.0] - 2026-05-27

Initial scaffold and repository bootstrap. No token-bucket logic yet — this
release establishes the structure, tooling, and quality gates the
implementation will be built on.

### Added

- `Cargo.toml` with full crate metadata, Rust 2024 edition, MSRV 1.85,
  dual `Apache-2.0 OR MIT` license, `docs.rs` configuration, and a
  size/perf-tuned release profile (`lto = "fat"`, `codegen-units = 1`,
  `panic = "abort"`, `strip`).
- Feature flags: `std` (default, propagates `std` to `clock-lib`) and
  `clock` (default, pluggable `clock-lib` time source with a mockable
  clock for deterministic tests).
- `src/lib.rs` crate root: the REPS lint posture (`deny(warnings)`,
  `deny(missing_docs)`, the `clippy` restriction set), `no_std`-on-
  demand wiring, crate-level documentation, the public `VERSION`
  constant, and a smoke test.
- `[lints.rust]` `check-cfg` registration for the `loom` and `docsrs`
  build cfgs so they stay quiet under `-D warnings`.
- `benches/bucket_bench.rs` — Criterion harness wired (`harness = false`)
  with a baseline for the packed-atomic CAS the acquire path is built on.
- `tests/loom_acquire.rs` — `loom` model-check harness, gated on
  `cfg(loom)`, asserting a shared-budget CAS never over-grants.
- Dev-dependencies for the test stack: `criterion` (benchmarks),
  `proptest` (invariant testing), and `loom` under `cfg(loom)` for
  concurrency model checking.
- `README.md` — overview, the "why better" positioning, Tier-1 quick
  start, configured-bucket and mockable-clock examples, design notes
  (lock-free hot path, lazy refill, the no-over-grant invariant),
  cross-platform support, and feature-flag reference.
- `docs/API.md` reference — Installation, the live `VERSION` constant,
  and the planned Tier 1/2/3 surface.
- `REPS.md` compliance baseline at the repository root.
- `.github/workflows/ci.yml` — Linux/macOS/Windows CI matrix on stable
  and MSRV (fmt, clippy `-D warnings`, test, `no_std` test, doc
  `-D warnings`), a `loom` model-check job, and a security job
  (`cargo audit` + `cargo deny check`).
- `.gitattributes` normalising line endings to LF and keeping
  development-only paths out of `git archive` tarballs.
- `clippy.toml` and `rustfmt.toml` lint/format configuration.
- `.dev/` AI-editor briefing (`PROMPT.md`, `ROADMAP.md`) — gitignored.

### Notes

- MSRV is **1.85** to match the Rust 2024 edition and the `clock-lib 1.0`
  dependency.
- Libraries do not commit `Cargo.lock` (per portfolio convention); it is
  gitignored.

[Unreleased]: https://github.com/jamesgober/better-bucket/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/jamesgober/better-bucket/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/jamesgober/better-bucket/compare/v0.9.5...v1.0.0
[0.9.5]: https://github.com/jamesgober/better-bucket/compare/v0.9.0...v0.9.5
[0.9.0]: https://github.com/jamesgober/better-bucket/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/jamesgober/better-bucket/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/jamesgober/better-bucket/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/jamesgober/better-bucket/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/jamesgober/better-bucket/compare/v0.3.0...v0.5.0
[0.3.0]: https://github.com/jamesgober/better-bucket/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jamesgober/better-bucket/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/better-bucket/releases/tag/v0.1.0
