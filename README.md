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
        <strong>better-bucket</strong> is a <b>lock-free token bucket</b> for Rust.
        The hot path — <code>try_acquire</code> — is <b>lock-free</b>, <b>allocation-free</b>, and <b>cache-aligned</b>: a single atomic compare-and-swap on one word. No background timer thread, no per-tick wakeups: refill is computed <em>lazily</em> from a monotonic clock the instant you ask.
        It runs the same on <b>Linux</b>, <b>macOS</b>, and <b>Windows</b>, and its caller-driven core runs in <code>no_std</code>.
    </p>
    <p>
        The common case is one line — <code>Bucket::per_second(100)</code> then <code>bucket.try_acquire(1)</code> — and that one-line path <em>is</em> the fast path. A builder, caller-supplied time, reservations, and a trait cover the rest without generic soup.
    </p>
    <p>
        The safety contract is the point: <b>the bucket never over-grants</b>. Under any interleaving of concurrent callers, the tokens granted over any window never exceed the capacity plus what the rate accrues over that window. The state is the single instant the bucket would have been empty, so a stale or backward clock reading can only deny, never grant. That is checked by <a href="https://github.com/tokio-rs/loom"><code>loom</code></a> with an advancing clock, a real-clock multi-thread grant-ceiling test, and <code>proptest</code>.
    </p>
</div>

> **Upgrade from 1.0.0.** 1.0.0 over-granted by one to two orders of magnitude
> under real-clock contention (a stale clock reading was read as a ~49.7-day
> gap). 1.1.0 fixes it with no API change. See the
> [CHANGELOG](./CHANGELOG.md#110--2026-10-13).

<hr>
<br>

<h2>What it does</h2>

- **Lock-free acquire.** A single `compare_exchange_weak` on one atomic word. No `Mutex`, no `RwLock`, no parking on the hot path.
- **Allocation-free.** No per-call operation allocates. A `Bucket<SystemClock>` is one 64-byte cache line with no heap tail.
- **Lazy refill.** Tokens accrue from elapsed monotonic time, computed on access. No timer thread, no wakeups, nothing spent while idle.
- **Never over-grants.** Stale readings, backward clocks, and races can only cause denials. Grants are exact token amounts; the only rounding is that a token may be granted a few nanoseconds before it is exactly due.
- **No wrap, no clamp.** Nanosecond time that never wraps (at least ~438 years of range), capacity exact for every `u32`, slow rates (one per day, one per year) refill on time.
- **Honest retry hints.** A denial carries the wait until the request would succeed; `time_until` gives the same answer without spending a token.
- **`no_std` core.** `BucketCore` takes the time from the caller and needs only `core` and a 64-bit atomic (`portable-atomic` covers targets without one).
- **One-line API.** The 80% case is a constructor and a method call.

<br>

## Features

- **Token bucket core** — lock-free `try_acquire` / `acquire` (one `compare_exchange_weak` on one atomic word), allocation-free, cache-line aligned to avoid false sharing between independent buckets
- **Lazy refill** — tokens accrue from monotonic elapsed time on access; no background threads, no timers
- **Overflow-safe math** — checked / saturating / widened arithmetic everywhere; no input or idle gap can wrap the state
- **Retry hints** — `Decision::Denied { retry_after }`, and the non-consuming `time_until(n)`
- **Caller-supplied time** — every clock-reading method has an `*_at(now)` twin; `BucketCore` drops the clock entirely for `no_std` and simulations
- **Reservations, refunds, partial grants** — `reserve(n, max_wait)` queues large requests fairly, `refund(n)` returns unused tokens, `acquire_up_to(n)` takes what is there
- **Deterministic tests** — inject a mockable clock (via `clock-lib`) and advance time without `sleep`
- **Tier-1 API** — `Bucket::per_second(n)` / `Bucket::per_duration(n, dur)` for the common case; `BucketConfig` and a builder for full control; a trait for the 1%
- **Optional** — `serde` for `BucketConfig`, `metrics` counters (zero cost when off), `portable-atomic` for targets without 64-bit atomics
- **Zero `unsafe`** — `#![forbid(unsafe_code)]`

<hr>
<br>

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
better-bucket = "1.1"

# no_std, caller-driven core (BucketCore + config/decision/error types):
better-bucket = { version = "1.1", default-features = false }

# no_std on a target without 64-bit atomics (e.g. Cortex-M):
better-bucket = { version = "1.1", default-features = false, features = ["portable-atomic"] }
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

<br>

## Configured Buckets (Tier 2)

When you need control over capacity, refill rate, and initial fill independently
— for example a large burst ceiling that refills slowly, or a bucket that starts
empty — use the builder:

```rust
use better_bucket::Bucket;
use std::time::Duration;

// 500-token burst ceiling, refilling 100 tokens/second, starting empty.
let bucket = Bucket::builder()
    .capacity(500)
    .refill(100, Duration::from_secs(1))
    .initial(0)
    .build()
    .expect("valid configuration");

// Try to take 10 tokens at once.
if bucket.try_acquire(10) {
    // allowed
}

// How many are available right now (after lazy refill).
let left = bucket.available();
```

`build()` validates the configuration (rejecting zero capacity, zero refill
amount, or zero refill period with a [`BucketError`]), so an invalid bucket can
never be constructed. `build_with_clock(clock)` takes a custom time source. If you
prefer to build the config value yourself, `BucketConfig::new` plus
`Bucket::from_config` — or `Bucket::with_config_and_clock(config, clock)` — is
the same path without the fluent surface. With the `serde` feature a
`BucketConfig` loads straight from a config file, with the same validation.

<br>

## Retry-After, Reservations, Refunds

```rust
use better_bucket::{Bucket, Decision};
use std::time::Duration;

let limiter = Bucket::per_second(10);

// A denial says how long until the same request would succeed...
if let Decision::Denied { retry_after } = limiter.acquire(20) {
    let _ = retry_after; // Duration::MAX: 20 can never fit a capacity of 10
}
// ...and time_until asks the same question without spending anything.
let _wait = limiter.time_until(5);

// A large request that small ones keep starving: reserve its place, then wait.
if let Some(wait) = limiter.reserve(10, Duration::from_secs(2)) {
    std::thread::sleep(wait); // or an async timer
}

// Work that failed downstream gives its tokens back.
limiter.refund(3);
```

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

<br>

## No Clock at All (`no_std`)

`BucketCore` is the same engine with the caller supplying the time as a
`Duration` since any epoch — a tick counter, a timer peripheral, a simulation
step:

```rust
use better_bucket::{BucketConfig, BucketCore};
use core::time::Duration;

let config = BucketConfig::new(5, 5, Duration::from_secs(1), 5).expect("valid");
let uplink = BucketCore::new(config, Duration::ZERO);

let now = Duration::from_millis(250);
if uplink.try_acquire_at(1, now) {
    // transmit
}
```

<hr>
<br>

## Design

### Lock-free, allocation-free hot path

The bucket's whole mutable state is one atomic word: the instant the bucket
would have been empty. `try_acquire(n)` is a `compare_exchange_weak` loop:

1. Read the clock and load the word.
2. Clamp an idle bucket to full, then charge `n` tokens: `next = max(z, now − capacity) + n`.
3. If `next` is after `now`, the tokens have not accrued: deny, without writing.
4. Otherwise CAS `next` into place; on failure (another thread won), retry
   against the value that won, with a CPU spin-loop hint.

There is no lock, no allocation, and no division on the grant path beyond the
monotonic clock read. Independent buckets sit on their own cache lines, so
unrelated limiters never falsely share.

### Why a stale clock reading is harmless

A grant only ever moves the empty-instant forward. A thread holding a clock
reading older than the last committed one — the race that made 1.0.0 over-grant
— computes `now − z` as smaller, so it sees fewer tokens and is denied. A clock
that steps backwards behaves the same way. There is no elapsed-time subtraction
that can wrap into a phantom refill, and no skew window to tune.

### Exact tokens, no wrap

Token amounts are counted in exact binary fractions of a token, so grants,
refunds and the capacity never round (with the clock frozen, every token is
conserved). The clock reading is converted to those units by one multiply and
one shift, rounded so an exactly-due token — five tokens 500 ms into a 10/s
bucket — is granted on time. Time is nanoseconds since construction and never
wraps; capacity is exact for every `u32`. The details, and every rounding
direction, are in [`docs/DESIGN.md`](./docs/DESIGN.md).

### How the no-over-grant contract is checked

- **`loom`** model-checks the real `Bucket` and `BucketCore` with the clock
  itself modelled as a shared atomic, so it explores every ordering of clock
  reads and clock advances against the CAS (1.0.0's model froze the clock and
  missed the bug; the new model finds it on the old code at once).
- **A real-clock contention test** runs 2–16 threads against the system clock
  and asserts total grants stay within capacity plus rate × elapsed.
- **Deterministic clock tests** stall a thread between its clock read and its
  CAS, and step a clock backwards.
- **`proptest`** drives arbitrary rates and arbitrary (including backward) time
  steps and checks every window of decisions, exact conservation, the exact
  grant boundary, and the honesty of retry hints and reservations.
- **An allocation audit** and **Miri** cover the rest.

<hr>
<br>

## Performance

The accounting is a load, a multiply-and-shift for the clock conversion, and a
CAS. On this release's test host (Ryzen 9 9950X3D, Windows 11) the bucket's own
work measures **~6.5 ns**, and `BucketCore::acquire_at` with a caller-supplied
reading **~4.5 ns**; a real `try_acquire` adds one monotonic clock read on top —
the dominant cost — for **~30–36 ns** single-threaded on Windows' QPC clock
(Linux `Instant` reads are cheaper). Full numbers, method, and caveats are in
[`docs/BENCHMARKS.md`](./docs/BENCHMARKS.md).

### vs `governor`

`governor` is **GCRA**: it tracks the same kind of single instant but answers only
allow/deny. `better-bucket` is a **token bucket**: multi-token acquires, a burst
capacity, an `available()` snapshot, `retry_after`, reservations, refunds, and
partial grants. On the same monotonic clock the two are close per call, and
`governor` is faster out of the box because its default `quanta` clock beats the
`Instant` clock `better-bucket` reads through `clock-lib`. Reach for `governor`
when a bare allow/deny is all you need, and for `better-bucket` when you want
token semantics. The [`ratelimit`](https://crates.io/crates/ratelimit) crate is
another lock-free token bucket; it has not been benchmarked here.

```bash
cargo bench --bench bucket_bench            # better-bucket baselines
cargo bench --features comparison           # + the governor comparison
```

<hr>
<br>

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `std`   | ✅      | Standard library. Off → `no_std`: `BucketCore`, `BucketConfig`, `BucketBuilder::build_config`, `Decision`, `BucketError`. |
| `clock` | ✅      | `Bucket`, `TokenBucket`, and the pluggable `clock-lib` time source (monotonic clock + mockable clock for tests). Implies `std` and `forge`. |
| `forge` | ✅ (via `clock`) | `error_forge::ForgeError` for `BucketError`. |
| `serde` | ❌      | `Serialize` / validating `Deserialize` for `BucketConfig`. |
| `metrics` | ❌    | Allowed / denied / granted-token counters (`metrics()`). Zero cost when off. |
| `portable-atomic` | ❌ | `portable-atomic` for the state word, for targets without native 64-bit atomics. |

The `comparison` feature is benchmark-only — it pulls in `governor` for the
head-to-head and is not part of the supported surface.

<br>


## Testing

```bash
# Unit + integration + property tests
cargo test
cargo test --features serde,metrics
cargo test --no-default-features              # the no_std core (tests themselves use std)
cargo build --no-default-features --features portable-atomic --target thumbv7em-none-eabihf

# Concurrency model checking (advancing clock, every interleaving)
RUSTFLAGS="--cfg loom" cargo test --release --test loom_acquire

# Miri (UB check)
cargo +nightly miri test --lib --test core_properties

# Benchmarks (add --features comparison for the governor head-to-head)
cargo bench --bench bucket_bench

# Format, lints, docs, and the security gates (all must be clean)
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cargo deny check
cargo audit
```

<br>

## Where It Fits

`better-bucket` is the single-purpose home for token-bucket math in the
wider library ecosystem. It is consumed by
[`rate-net`](https://github.com/jamesgober/rate-net) — a multi-algorithm,
per-key rate limiter — which uses this crate as its token-bucket strategy
rather than reimplementing the algorithm. `better-bucket` stays
foreign-compatible: it works perfectly well on its own, with no obligation to
pull in the rest of the family.

<br>
<br>

<div align="center">
    <a href="./docs/DESIGN.md" title="How the bucket works"><img width="180px" height="auto" alt="rust bucket" src="./_assets/bucket.png"></a>
</div>

<br>
<br>

## Cross-Platform Support

**Tier 1 Support:**
- ✅ Linux (x86_64, aarch64)
- ✅ macOS (Apple Silicon; x86_64 builds from the same code but is not in CI)
- ✅ Windows (x86_64)

Behavior is identical across all of them. CI runs Linux x86_64 and aarch64,
macOS on Apple Silicon, and Windows x86_64 on stable and MSRV, plus a 32-bit
Linux test job and a `no_std` build for `thumbv7em-none-eabihf`. A commit that
breaks any platform is a broken commit.

<br>

## Documentation

- [**API Reference**](./docs/API.md) — every public item, parameters, examples.
- [**Design**](./docs/DESIGN.md) — the single-instant state, exact token units,
  the clock conversion and its rounding, and how the no-over-grant contract is
  checked.
- [**Benchmarks**](./docs/BENCHMARKS.md) — numbers, method, and the `governor`
  comparison.
- [**Stability**](./docs/STABILITY.md) — the frozen `1.x` surface and the SemVer
  contract.
- [**CHANGELOG**](./CHANGELOG.md) — per-release detail.

<br>

## Standards

- **REPS** (**Rust Efficiency** &amp; **Performance Standards**). See [REPS.md](REPS.md).
- **MSRV:** Rust 1.85.
- **Edition:** 2024.

<br>



## Contributing

Contributions are welcome. 

Before opening a PR, make sure everything is clean, and that any change touching the acquire path is accompanied by a benchmark and a `loom` test (where it affects concurrency).

```bash
# Format + lints
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings

# Run tests
cargo test --all-features
```


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
