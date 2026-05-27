# better-bucket — API Reference

> Complete reference for every public item in `better-bucket`, with examples.
> Format mirrors the portfolio standard ([metrics-lib API.md](https://github.com/jamesgober/metrics-lib/blob/main/docs/API.md)).
>
> **Status: pre-1.0.** This document tracks the API surface as it lands across
> the `0.x` series. `0.1.0` is the scaffold; the only item it actually ships is
> the [`VERSION`](#version) constant. Sections marked _(planned)_ describe the
> intended surface and are filled in as each roadmap phase ships — the
> foundation types in `0.2.0`, the lock-free core in `0.3.0`.

## Table of Contents

- [Installation](#installation)
- [Overview](#overview)
- [Crate metadata](#crate-metadata)
  - [`VERSION`](#version)
- [Tier 1 — the lazy path](#tier-1--the-lazy-path)
  - [`Bucket::per_second`](#bucketper_second) _(planned: 0.2)_
  - [`Bucket::per_duration`](#bucketper_duration) _(planned: 0.2)_
  - [`Bucket::try_acquire`](#buckettry_acquire) _(planned: 0.2)_
  - [`Bucket::acquire`](#bucketacquire) _(planned: 0.2)_
  - [`Bucket::available`](#bucketavailable) _(planned: 0.2)_
- [Tier 2 — the configured path](#tier-2--the-configured-path)
  - [`Bucket::builder`](#bucketbuilder) _(planned: 0.5)_
  - [`BucketBuilder`](#bucketbuilder-type) _(planned: 0.5)_
  - [`Bucket::with_clock`](#bucketwith_clock) _(planned: 0.5)_
- [Tier 3 — the power path](#tier-3--the-power-path)
  - [`TokenBucket` trait](#tokenbucket-trait) _(planned: 0.2)_
- [Errors](#errors) _(planned: 0.2)_
- [Feature flags](#feature-flags)

---

## Installation

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
better-bucket = "0.1"
```

For a `no_std` build — the standard library is dropped and the caller supplies
time explicitly rather than relying on the bundled clock:

```toml
[dependencies]
better-bucket = { version = "0.1", default-features = false }
```

MSRV is **1.85** (Rust 2024 edition).

---

## Overview

`better-bucket` exposes a lock-free, allocation-free token bucket. The common
case is a constructor plus `try_acquire`; advanced use is a builder plus an
optional injected clock; the full surface is the `TokenBucket` trait.

The hot path never allocates and never locks. Refill is lazy — tokens accrue
from monotonic elapsed time when you call `try_acquire` / `available`. The
core safety guarantee is that the bucket **never over-grants** under any
concurrent interleaving.

```text
use better_bucket::Bucket;

let bucket = Bucket::per_second(100);
if bucket.try_acquire(1) {
    // allowed
}
```

> The block above shows the target surface; the token types land in `0.2.0`.
> The section below documents what `0.1.0` actually exports.

---

## Crate metadata

### `VERSION`

```rust
pub const VERSION: &str;
```

The version of the linked `better-bucket` crate, captured from `Cargo.toml` at
compile time via `env!("CARGO_PKG_VERSION")`.

**Why it exists.** A consumer sitting several layers up a dependency tree often
needs to report or assert the exact bucket build it links against — for startup
diagnostics, bug reports, or guarding against version skew between a service and
a sidecar that must agree on rate-limit semantics. Reading the constant is free
and needs no instance.

**Returns.** A `&'static str` in `MAJOR.MINOR.PATCH` form (plus any pre-release
suffix). Always non-empty.

**Examples**

Print the linked version at startup:

```rust
println!("better-bucket {}", better_bucket::VERSION);
```

Assert a minimum series in an integration test or a build-time check:

```rust
let version = better_bucket::VERSION;
assert_eq!(version.split('.').count(), 3); // major.minor.patch

let major = version.split('.').next().unwrap_or("0");
assert_eq!(major, "0", "expected a 0.x release");
```

---

## Tier 1 — the lazy path

_The one-line, zero-ceremony surface for the ~80% case. Documented in full as
the 0.2 foundation release lands. Intended signatures:_

- `Bucket::per_second(n: u32) -> Bucket` — a bucket of capacity `n` that
  refills `n` tokens per second.
- `Bucket::per_duration(n: u32, period: Duration) -> Bucket` — `n` tokens per
  arbitrary period.
- `Bucket::try_acquire(&self, n: u32) -> bool` — take `n` tokens if available;
  never blocks, never allocates.
- `Bucket::acquire(&self, n: u32) -> bool` — alias semantics documented at 0.2.
- `Bucket::available(&self) -> u32` — tokens available right now (after lazy
  refill).

---

## Tier 2 — the configured path

_Builder surface for capacity / refill / burst / initial fill and clock
injection. Documented in full at the 0.5 feature-complete release._

---

## Tier 3 — the power path

_The `TokenBucket` trait — the surface `rate-net` consumes. Documented as the
trait stabilises at 0.2._

---

## Errors

_Construction-time validation (zero capacity / zero rate) returns a
domain-specific error built on `error-forge`. The acquire path itself is
infallible and returns a plain allow/deny outcome. Variants documented at
0.2._

---

## Feature flags

| Feature | Default | Description |
|---------|---------|-------------|
| `std`   | yes     | Standard library. Off → `no_std`, caller drives time. Propagates `std` to `clock-lib` when `clock` is also on. |
| `clock` | yes     | Pluggable [`clock-lib`](https://crates.io/crates/clock-lib) time source + mockable clock for tests. |

Disabling default features yields a pure `no_std`, caller-driven-tick build
with no `clock-lib` dependency:

```toml
better-bucket = { version = "0.1", default-features = false }
```

---

<sub>Copyright &copy; 2026 <strong>James Gober</strong>. All rights reserved.</sub>
