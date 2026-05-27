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

[Unreleased]: https://github.com/jamesgober/better-bucket/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/jamesgober/better-bucket/releases/tag/v0.1.0
