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

[Unreleased]: https://github.com/jamesgober/better-bucket/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/jamesgober/better-bucket/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/better-bucket/releases/tag/v0.1.0
