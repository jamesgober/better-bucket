<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <b>better-bucket</b>
    <br>
    <sub>
        <sup>A BETTER TOKEN BUCKET</sup>
    </sub>
</h1>

<div align="center">
    <a href="https://crates.io/crates/better-bucket"><img alt="Crates.io" src="https://img.shields.io/crates/v/better-bucket"></a>
    <a href="https://crates.io/crates/better-bucket" alt="Download better-bucket"><img alt="Crates.io Downloads" src="https://img.shields.io/crates/d/better-bucket?color=%230099ff"></a>
    <a href="https://docs.rs/better-bucket" title="better-bucket Documentation"><img alt="docs.rs" src="https://img.shields.io/docsrs/better-bucket"></a>
    <a href="https://github.com/jamesgober/better-bucket/actions"><img alt="GitHub CI" src="https://github.com/jamesgober/better-bucket/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md" title="MSRV"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.85%2B-blue"></a>
</div>

<br>

<div align="left">
    <p>
        <strong>better-bucket</strong> is a <b>genuinely better token bucket</b> for Rust. 
        The hot path — <code>try_acquire</code> — is <b>lock-free</b>, <b>allocation-free</b>, and <b>cache-aligned</b>, built on a single atomic compare-and-swap over a packed token/tick word. No background timer thread, no per-tick wakeups: refill is computed <em>lazily</em> from a monotonic clock the instant you ask. 
        It is engineered for <b>maximum throughput</b>, <b>minimum overhead</b>, and <b>correctness under brutal contention</b> across <b>Linux</b>, <b>macOS</b>, and <b>Windows</b>.
    </p>
    <p>
        Most "token bucket" crates make you choose between speed and a sane API. This one refuses the trade. The common case is one line — <code>Bucket::per_second(100)</code> then <code>bucket.try_acquire(1)</code> — and that one-line path <em>is</em> the fast path. Power users get a builder and a full trait surface; nobody is forced through generic soup to rate-limit a loop.
    </p>
    <p>
        The safety contract is the headline feature: <b>the bucket never over-grants</b>. Across any concurrent interleaving, the total tokens handed out never exceed capacity plus accrued refill. That invariant is defended by <a href="https://github.com/tokio-rs/loom"><code>loom</code></a> model checking and <code>proptest</code>, not by hope.
    </p>
    <br>
    <hr>
    <p>
        <strong>MSRV is 1.85+</strong> (Rust 2024 edition). Zero <code>unsafe</code> on the public path. <code>no_std</code>-capable.
    </p>
    <blockquote>
        <strong>Status: pre-1.0, in active development.</strong> <code>0.2.0</code> is the <strong>foundation</strong> release: the public surface — <code>Bucket</code>, <code>BucketConfig</code>, <code>Decision</code>, <code>BucketError</code>, and the <code>TokenBucket</code> trait — is in place and exercised by property tests, on a simple, correct implementation. The lock-free, allocation-free, cache-aligned core that earns the crate its name replaces the internals in <code>0.3.0</code> <em>without changing this surface</em>; <code>1.0.0</code> is the API freeze. See <a href="./CHANGELOG.md"><code>CHANGELOG.md</code></a> for per-release detail.
    </blockquote>
</div>


<hr>
<br>

<h2>Why "better"?</h2>

A token bucket is simple to get working and surprisingly hard to get <em>right</em> — most implementations leak performance to a lock, leak correctness under contention, or leak ergonomics behind a generic builder. `better-bucket` targets all three at once:

- **Lock-free acquire.** A single `compare_exchange_weak` on a packed `(tokens, last_refill_tick)` word. No `Mutex`, no `RwLock`, no parking on the hot path.
- **Allocation-free steady state.** Acquiring never allocates. A bucket is a small, cache-line-aligned value with no heap tail.
- **Lazy refill.** Tokens accrue from elapsed monotonic time, computed on access. No timer thread burning a core, no wakeups, no watts spent while idle.
- **Overflow-safe.** Every refill and capacity computation is checked or saturating. A hostile request count or a multi-day idle gap can't wrap the counter or over-fill the bucket.
- **Never over-grants.** The core safety invariant, proven under `loom` and `proptest`.
- **One-line API.** The 80% case is a constructor and a method call. No ceremony.

<br>
<hr>
<br>

## Features

- **Token bucket core** — lock-free `try_acquire` / `acquire`, packed atomic state, cache-line aligned to avoid false sharing between independent buckets
- **Lazy refill** — tokens accrue from monotonic elapsed time on access; no background threads, no timers
- **Overflow-safe math** — checked / saturating arithmetic on every refill and capacity path
- **Deterministic tests** — inject a mockable clock (via `clock-lib`) and advance time without `sleep`
- **Tier-1 API** — `Bucket::per_second(n)` / `Bucket::per_duration(n, dur)` for the common case; a builder for control; a trait for the 1%
- **No over-grant guarantee** — verified with `loom` model checking and `proptest`
- **`no_std`-capable** — the lock-free core runs without the standard library (caller drives time)
- **Zero `unsafe`** on the public path

<br>
<hr>
<br>

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
better-bucket = "0.2"

# no_std build (no clock-lib; caller-driven core lands in 0.3):
better-bucket = { version = "0.2", default-features = false }
```

<hr>
<br>

## Quick Start

```rust
use better_bucket::Bucket;

// 100 tokens per second, bucket holds up to 100.
let bucket = Bucket::per_second(100);

// The 80% case: one call. Returns true if a token was available.
if bucket.try_acquire(1) {
    // allowed — do the work
} else {
    // denied — shed load / return 429 / back off
}
```

That is the whole common case. No builder, no type parameters, no setup.

<hr>
<br>

## Configured Buckets (Tier 2)

When you need control over capacity, refill rate, and initial fill independently
— for example a large burst ceiling that refills slowly, or a bucket that starts
empty — build a validated [`BucketConfig`] and hand it to `Bucket::from_config`:

```rust
use better_bucket::{Bucket, BucketConfig};
use std::time::Duration;

// 500-token burst ceiling, refilling 100 tokens/second, starting empty.
let config = BucketConfig::new(500, 100, Duration::from_secs(1), 0)
    .expect("valid configuration");
let bucket = Bucket::from_config(config);

// Try to take 10 tokens at once.
if bucket.try_acquire(10) {
    // allowed
}

// How many are available right now (after lazy refill).
let left = bucket.available();
```

`BucketConfig::new` rejects nonsensical configurations (zero capacity, zero
refill amount, zero refill period) up front with a [`BucketError`], so an
invalid bucket can never be constructed. A fluent `Bucket::builder()` is a
planned Tier-2 convenience for `0.5.0`; the config path above is its foundation.

<hr>
<br>

## Deterministic Testing (mockable clock)

Time-driven code is normally a pain to test — you end up sprinkling `sleep`
through the suite and hoping. `better-bucket` lets you inject a manual clock
from [`clock-lib`](https://crates.io/crates/clock-lib) and advance time
instantly:

```rust
use better_bucket::Bucket;
use clock_lib::ManualClock;
use std::sync::Arc;
use std::time::Duration;

// Share one clock between the test and the bucket via `Arc`.
let clock = Arc::new(ManualClock::new());
let bucket = Bucket::per_second(10).with_clock(Arc::clone(&clock));

// Drain the bucket.
assert!(bucket.try_acquire(10));
assert!(!bucket.try_acquire(1)); // empty

// Advance one second — no real sleep, fully deterministic.
clock.advance(Duration::from_secs(1));
assert!(bucket.try_acquire(10)); // refilled
```

<hr>
<br>

## Design

> **Implementation status.** `0.2.0` ships the public surface on a simple,
> correct implementation (a mutex around millitoken accounting) so the API and
> the safety invariants can be locked and property-tested first. The lock-free,
> allocation-free design described below replaces those internals in `0.3.0`
> behind the *same* public surface — no API change, no recompile for consumers.

### Lock-free, allocation-free hot path

The bucket packs its mutable state — current tokens and the last-refill
tick — into a single atomic word. `try_acquire` is a `compare_exchange_weak`
loop:

1. Load the packed word.
2. Compute lazy refill from monotonic elapsed time (saturating).
3. If enough tokens, CAS the new `(tokens - n, now_tick)` in place.
4. On CAS failure (another thread won the race), retry with bounded backoff.

There is no lock, no allocation, and no syscall on the success path beyond the
monotonic clock read. Independent buckets sit on their own cache lines, so
unrelated limiters never falsely share.

### Lazy refill, no timer thread

Refill is never pushed by a background thread. Tokens are computed from the
elapsed monotonic time at the moment you call `try_acquire` / `available`.
An idle bucket costs nothing — no wakeups, no spinning, no watts.

### The no-over-grant invariant

The defining correctness property: **across any concurrent interleaving, the
total tokens granted never exceed capacity plus the tokens legitimately
accrued by refill.** This is the property that separates a correct rate
limiter from a leaky one, and it is verified two ways:

- **`loom`** exhaustively explores the CAS interleavings of concurrent
  `try_acquire` calls and asserts no lost update and no over-grant.
- **`proptest`** throws arbitrary sequences of acquires and time advances at
  the bucket and asserts tokens always stay in `[0, capacity]` and grants
  never exceed what refill allows.

<hr>
<br>

## Performance

The acquire path is designed to land in single-digit nanoseconds on a held
bucket, competitive with or ahead of the established token-bucket crates.
Benchmark numbers (single-thread `try_acquire`, contended multi-thread
acquire, refill-after-idle) are produced by the Criterion suite:

```bash
cargo bench --bench bucket_bench
```

A head-to-head comparison against `governor` and `leaky-bucket` ships with the
performance write-up as the suite matures toward 1.0. Numbers are recorded
honestly, including any case not won.

<hr>
<br>

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `std`   | ✅      | Standard library. Off → `no_std`. |
| `clock` | ✅      | Pluggable `clock-lib` time source: monotonic clock + mockable clock for tests. Implies `std` (clock-lib's `Clock` is std-gated). |

```toml
# no_std build (no clock-lib):
better-bucket = { version = "0.2", default-features = false }
```

> A bare `no_std` build currently exposes only the crate's `VERSION`; the
> `Bucket` surface needs the `clock` feature. The `no_std`-capable, caller-driven
> core (no `std`, no clock dependency) lands with the lock-free rewrite in
> `0.3.0`.

<hr>
<br>

## Cross-Platform Support

**Tier 1 Support:**
- ✅ Linux (x86_64, aarch64)
- ✅ macOS (x86_64, Apple Silicon)
- ✅ Windows (x86_64)

Behavior is identical across all three; the CI matrix runs every target on
stable and MSRV. A commit that breaks any platform is a broken commit.

<hr>
<br>

## Testing

```bash
# Unit + integration + property tests
cargo test --all-features

# Concurrency model checking (no over-grant under interleaving)
RUSTFLAGS="--cfg loom" cargo test --test loom_acquire

# Benchmarks
cargo bench --bench bucket_bench

# Format + lints (must be clean)
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

<hr>
<br>

## Where It Fits

`better-bucket` is the single-purpose home for token-bucket math in the
wider library ecosystem. It is consumed by
[`rate-net`](https://github.com/jamesgober/rate-net) — a multi-algorithm,
per-key rate limiter — which uses this crate as its token-bucket strategy
rather than reimplementing the algorithm. `better-bucket` stays
foreign-compatible: it works perfectly well on its own, with no obligation to
pull in the rest of the family.

<hr>
<br>

## Contributing

Contributions are welcome. Before opening a PR, make sure `cargo fmt`,
`cargo clippy --all-targets --all-features -- -D warnings`, and
`cargo test --all-features` are all clean, and that any change touching the
acquire path is accompanied by a benchmark and (where it affects concurrency)
a `loom` test.

<hr>
<br>

<!-- LICENSE
############################################# -->
<div id="license">
    <h2>⚖️ License</h2>
    <p>Licensed under either of</p>
    <ul>
        <li><b>Apache License, Version 2.0</b> — see <a href="./LICENSE-APACHE">LICENSE-APACHE</a> (<a href="http://www.apache.org/licenses/LICENSE-2.0" target="_blank">http://www.apache.org/licenses/LICENSE-2.0</a>)</li>
        <li><b>MIT License</b> — see <a href="./LICENSE-MIT">LICENSE-MIT</a> (<a href="http://opensource.org/licenses/MIT" target="_blank">http://opensource.org/licenses/MIT</a>)</li>
    </ul>
    <p>at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.</p>
</div>

<!-- FOOT COPYRIGHT
################################################# -->
<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2026 <strong>JAMES GOBER.</strong></sup>
</div>
