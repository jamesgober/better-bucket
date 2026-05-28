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

### Added

### Changed

### Fixed

### Security

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

[Unreleased]: https://github.com/jamesgober/better-bucket/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/jamesgober/better-bucket/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/jamesgober/better-bucket/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/jamesgober/better-bucket/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/jamesgober/better-bucket/compare/v0.3.0...v0.5.0
[0.3.0]: https://github.com/jamesgober/better-bucket/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jamesgober/better-bucket/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/better-bucket/releases/tag/v0.1.0
