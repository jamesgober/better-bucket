# better-bucket — API Reference

> Complete reference for every public item in `better-bucket`, with examples.
> Format mirrors the portfolio standard ([metrics-lib API.md](https://github.com/jamesgober/metrics-lib/blob/main/docs/API.md)).
>
> **Status: pre-1.0.** The `0.3` release ships the lock-free core (a single
> `compare_exchange_weak` on a packed atomic word, allocation-free) behind the
> public surface documented below — unchanged from the `0.2` foundation.
> Items marked _(planned)_ are not yet shipped.

## Table of Contents

- [Installation](#installation)
- [Overview](#overview)
- [Tier 1 — the lazy path](#tier-1--the-lazy-path)
  - [`Bucket::per_second`](#bucketper_second)
  - [`Bucket::per_duration`](#bucketper_duration)
  - [`Bucket::try_acquire`](#buckettry_acquire)
  - [`Bucket::acquire`](#bucketacquire)
  - [`Bucket::available`](#bucketavailable)
  - [`Bucket::capacity`](#bucketcapacity)
  - [`Bucket::reset`](#bucketreset)
- [Tier 2 — the configured path](#tier-2--the-configured-path)
  - [`BucketConfig`](#bucketconfig)
  - [`Bucket::from_config`](#bucketfrom_config)
  - [`Bucket::with_clock`](#bucketwith_clock)
  - [`Bucket::config`](#bucketconfig-method)
  - [`Bucket::builder`](#bucketbuilder) _(planned: 0.5)_
- [Tier 3 — the power path](#tier-3--the-power-path)
  - [`TokenBucket` trait](#tokenbucket-trait)
- [Types](#types)
  - [`Decision`](#decision)
  - [`BucketError`](#bucketerror)
- [Crate metadata](#crate-metadata)
  - [`VERSION`](#version)
- [Feature flags](#feature-flags)

---

## Installation

```toml
[dependencies]
better-bucket = "0.3"
```

`no_std` build (exposes only [`VERSION`](#version); the `Bucket` surface needs
the default `clock` feature, which implies `std`):

```toml
[dependencies]
better-bucket = { version = "0.3", default-features = false }
```

MSRV is **1.85** (Rust 2024 edition).

### Limits

State is one `AtomicU64` packing tokens (millitokens, upper 32 bits) and
milliseconds since construction (lower 32 bits). Two consequences:

- Capacity is effectively capped at **~4.29 million tokens** (`u32::MAX`
  millitokens); larger values are clamped.
- `retry_after` is reported at **millisecond** resolution, and the millisecond
  counter saturates after **~49.7 days** of clock advance, after which refill
  stalls until [`reset`](#bucketreset) re-anchors it.

---

## Overview

`better-bucket` exposes a single token bucket behind a small surface:

- **Tier 1** — a constructor (`per_second` / `per_duration`) plus `try_acquire`.
  The 80% case, one line each.
- **Tier 2** — a validated [`BucketConfig`] for full control, plus clock
  injection via [`with_clock`](#bucketwith_clock).
- **Tier 3** — the [`TokenBucket`](#tokenbucket-trait) trait, the abstraction a
  consumer (e.g. `rate-net`) codes against.

The acquire path is infallible — it returns a [`Decision`](#decision), never a
`Result`. The only fallible operation is building a [`BucketConfig`](#bucketconfig),
which rejects unworkable parameters at construction. Refill is lazy: tokens
accrue from the monotonic clock when you call an accessor, never from a
background thread.

```rust
use better_bucket::Bucket;

let bucket = Bucket::per_second(100);
if bucket.try_acquire(1) {
    // allowed
}
```

---

## Tier 1 — the lazy path

### `Bucket::per_second`

```rust
pub fn per_second(rate: u32) -> Bucket<SystemClock>
```

Creates a bucket of capacity `rate` that refills `rate` tokens per second,
starting full, driven by the OS monotonic clock. This is the headline
constructor.

**Parameters**

- `rate` — both the capacity (burst ceiling) and the per-second refill amount. A
  `rate` of `0` yields a bucket that grants nothing (capacity `0`); use
  [`BucketConfig::new`](#bucketconfig) when you want `0` rejected as an error.

**Returns** a `Bucket<SystemClock>`, ready to use.

**Examples**

```rust
use better_bucket::Bucket;

let bucket = Bucket::per_second(50);
assert_eq!(bucket.capacity(), 50);
assert!(bucket.try_acquire(1));
```

Draining and refilling (with an injected clock for determinism):

```rust
use better_bucket::Bucket;
use clock_lib::ManualClock;
use std::sync::Arc;
use std::time::Duration;

let clock = Arc::new(ManualClock::new());
let bucket = Bucket::per_second(10).with_clock(Arc::clone(&clock));

assert!(bucket.try_acquire(10)); // drain
assert!(!bucket.try_acquire(1)); // empty
clock.advance(Duration::from_secs(1));
assert_eq!(bucket.available(), 10); // refilled
```

### `Bucket::per_duration`

```rust
pub fn per_duration(amount: u32, period: Duration) -> Bucket<SystemClock>
```

Creates a bucket of capacity `amount` that refills `amount` tokens every
`period`, starting full. Use this when the natural rate is not per-second.

**Parameters**

- `amount` — capacity and the per-`period` refill amount.
- `period` — the period over which `amount` accrues.

A zero `amount` or zero `period` yields a bucket that grants nothing.

**Examples**

```rust
use better_bucket::Bucket;
use std::time::Duration;

// 5 tokens every 100ms.
let bucket = Bucket::per_duration(5, Duration::from_millis(100));
assert_eq!(bucket.capacity(), 5);
```

```rust
use better_bucket::Bucket;
use std::time::Duration;

// 1000 tokens per minute.
let bucket = Bucket::per_duration(1000, Duration::from_secs(60));
assert!(bucket.try_acquire(1000));
```

### `Bucket::try_acquire`

```rust
pub fn try_acquire(&self, n: u32) -> bool
```

Attempts to take `n` tokens, returning whether it succeeded. The one-line
convenience over [`acquire`](#bucketacquire) — equivalent to
`self.acquire(n).is_allowed()`. Never blocks, never allocates.

**Parameters**

- `n` — tokens to take. `0` always succeeds; more than the capacity always
  fails.

**Returns** `true` if `n` tokens were available and have been deducted; `false`
otherwise (the bucket is left untouched).

**Examples**

```rust
use better_bucket::Bucket;

let bucket = Bucket::per_second(1);
assert!(bucket.try_acquire(1));
assert!(!bucket.try_acquire(1)); // drained
```

Admission control:

```rust
use better_bucket::Bucket;

let limiter = Bucket::per_second(100);
fn handle() {}

if limiter.try_acquire(1) {
    handle();
} else {
    // shed load: return 429, drop, or back off
}
```

### `Bucket::acquire`

```rust
pub fn acquire(&self, n: u32) -> Decision
```

Attempts to take `n` tokens, returning the full [`Decision`](#decision). On
success the tokens are deducted and the result is `Decision::Allowed`. On
failure the bucket is untouched and the result is `Decision::Denied`, carrying
the minimum `retry_after` until the same request would succeed.

**Parameters**

- `n` — tokens to take.

**Returns** a [`Decision`](#decision). Requesting more than the capacity returns
`Denied { retry_after: Duration::MAX }` (it can never succeed).

**Examples**

```rust
use better_bucket::{Bucket, Decision};

let bucket = Bucket::per_second(5);
assert_eq!(bucket.acquire(3), Decision::Allowed);
assert_eq!(bucket.available(), 2);
```

Using the retry hint:

```rust
use better_bucket::{Bucket, Decision};

let bucket = Bucket::per_second(10);
match bucket.acquire(20) {
    Decision::Allowed => { /* serve */ }
    Decision::Denied { retry_after } => {
        // populate a Retry-After header from `retry_after`
        let _ = retry_after;
    }
    _ => {}
}
```

### `Bucket::available`

```rust
pub fn available(&self) -> u32
```

Returns how many whole tokens are available right now, after applying lazy
refill. Reading `available` brings the bucket current the same way an acquire
does. Under concurrent acquires this is a momentary snapshot — treat it as
advisory, not a reservation.

**Examples**

```rust
use better_bucket::Bucket;

let bucket = Bucket::per_second(10);
assert_eq!(bucket.available(), 10);
assert!(bucket.try_acquire(4));
assert_eq!(bucket.available(), 6);
```

### `Bucket::capacity`

```rust
pub const fn capacity(&self) -> u32
```

Returns the bucket's capacity — its burst ceiling, the maximum tokens it can
hold.

**Examples**

```rust
use better_bucket::Bucket;

assert_eq!(Bucket::per_second(64).capacity(), 64);
```

### `Bucket::reset`

```rust
pub fn reset(&self)
```

Refills the bucket to full and re-anchors its internal millisecond counter to
the current time. Two uses: discard accumulated debt to grant a fresh burst,
and keep refill alive on a process that runs longer than the ~49.7-day
saturation window (call `reset` periodically).

**Examples**

```rust
use better_bucket::Bucket;

let bucket = Bucket::per_second(4);
assert!(bucket.try_acquire(4));
assert_eq!(bucket.available(), 0);
bucket.reset();
assert_eq!(bucket.available(), 4);
```

---

## Tier 2 — the configured path

### `BucketConfig`

```rust
pub struct BucketConfig { /* private */ }

pub fn new(capacity: u32, refill_amount: u32, refill_period: Duration, initial: u32)
    -> Result<BucketConfig, BucketError>
pub const fn capacity(&self) -> u32
pub const fn refill_amount(&self) -> u32
pub const fn refill_period(&self) -> Duration
pub const fn initial(&self) -> u32
```

The validated parameters that define a bucket: a `capacity` (burst ceiling), a
sustained rate of `refill_amount` tokens per `refill_period`, and an `initial`
fill. Construct one with `new`; the Tier-1 constructors build one for you for
the common case.

**`new` parameters**

- `capacity` — maximum tokens held. Must be `> 0`.
- `refill_amount` — tokens added each `refill_period`. Must be `> 0`.
- `refill_period` — the accrual period. Must be non-zero.
- `initial` — starting tokens, clamped to `capacity`.

**`new` errors**

- [`BucketError::ZeroCapacity`](#bucketerror) — `capacity` was `0`.
- [`BucketError::ZeroRefillAmount`](#bucketerror) — `refill_amount` was `0`.
- [`BucketError::ZeroRefillPeriod`](#bucketerror) — `refill_period` was zero.

**Examples**

```rust
use better_bucket::BucketConfig;
use std::time::Duration;

let config = BucketConfig::new(500, 100, Duration::from_secs(1), 0)?;
assert_eq!(config.capacity(), 500);
assert_eq!(config.refill_amount(), 100);
assert_eq!(config.initial(), 0);
# Ok::<(), better_bucket::BucketError>(())
```

`initial` above `capacity` is clamped, not rejected:

```rust
use better_bucket::BucketConfig;
use std::time::Duration;

let config = BucketConfig::new(100, 100, Duration::from_secs(1), 999)?;
assert_eq!(config.initial(), 100);
# Ok::<(), better_bucket::BucketError>(())
```

Rejection of a nonsensical configuration:

```rust
use better_bucket::{BucketConfig, BucketError};
use std::time::Duration;

let err = BucketConfig::new(0, 10, Duration::from_secs(1), 0).unwrap_err();
assert_eq!(err, BucketError::ZeroCapacity);
```

### `Bucket::from_config`

```rust
pub fn from_config(config: BucketConfig) -> Bucket<SystemClock>
```

Creates a bucket from a validated [`BucketConfig`](#bucketconfig), driven by the
OS monotonic clock. Use it when capacity, rate, and initial fill differ — e.g. a
large burst ceiling with a slow refill, or a bucket that starts empty.

**Examples**

```rust
use better_bucket::{Bucket, BucketConfig};
use std::time::Duration;

// Burst up to 500, refill 100/sec, start empty.
let config = BucketConfig::new(500, 100, Duration::from_secs(1), 0)?;
let bucket = Bucket::from_config(config);
assert_eq!(bucket.available(), 0);
assert_eq!(bucket.capacity(), 500);
# Ok::<(), better_bucket::BucketError>(())
```

### `Bucket::with_clock`

```rust
pub fn with_clock<C2: Clock>(self, clock: C2) -> Bucket<C2>
```

Replaces the bucket's time source, resetting it to its initial fill anchored at
the new clock's current reading. This is the clock-injection seam; the intended
use is immediately after construction, chiefly in tests, where a
[`ManualClock`](https://docs.rs/clock-lib) makes refill deterministic with no
`sleep`.

**Parameters**

- `clock` — any [`Clock`](https://docs.rs/clock-lib) implementation. A shared
  `ManualClock` is typically passed as `Arc<ManualClock>` (clock-lib implements
  `Clock` for `Arc<C>`), so the test driver keeps a handle to advance it.

**Examples**

```rust
use better_bucket::Bucket;
use clock_lib::ManualClock;
use std::sync::Arc;
use std::time::Duration;

let clock = Arc::new(ManualClock::new());
let bucket = Bucket::per_second(10).with_clock(Arc::clone(&clock));

assert!(bucket.try_acquire(10));
clock.advance(Duration::from_millis(500)); // half a period
assert_eq!(bucket.available(), 5);
```

### `Bucket::config` <a id="bucketconfig-method"></a>

```rust
pub const fn config(&self) -> BucketConfig
```

Returns the [`BucketConfig`](#bucketconfig) the bucket was built from, for
introspection.

**Examples**

```rust
use better_bucket::Bucket;
use std::time::Duration;

let bucket = Bucket::per_second(10);
assert_eq!(bucket.config().refill_period(), Duration::from_secs(1));
```

### `Bucket::builder` _(planned: 0.5)_

A fluent builder (`Bucket::builder().capacity(..).refill(..).initial(..).build()`)
is planned for the `0.5` feature-complete release as a convenience over
[`BucketConfig::new`](#bucketconfig). The config path is its foundation and is
available now.

---

## Tier 3 — the power path

### `TokenBucket` trait

```rust
pub trait TokenBucket {
    fn acquire(&self, n: u32) -> Decision;
    fn try_acquire(&self, n: u32) -> bool;
    fn available(&self) -> u32;
    fn capacity(&self) -> u32;
}
```

The abstraction a consumer codes against, so it can hold a bucket without naming
its concrete clock type. Implemented for every `Bucket<C>`. The methods mirror
the inherent methods of [`Bucket`] exactly; see those for each contract. The
trait is object-safe — `&dyn TokenBucket` works.

**Examples**

Holding a bucket behind the trait:

```rust
use better_bucket::{Bucket, TokenBucket};

fn drain(bucket: &dyn TokenBucket) -> u32 {
    let mut taken = 0;
    while bucket.try_acquire(1) {
        taken += 1;
    }
    taken
}

let bucket = Bucket::per_second(5);
assert_eq!(drain(&bucket), 5);
```

Generic over the clock:

```rust
use better_bucket::{Bucket, TokenBucket};
use clock_lib::Clock;

fn capacity_of<C: Clock>(bucket: &Bucket<C>) -> u32 {
    TokenBucket::capacity(bucket)
}

assert_eq!(capacity_of(&Bucket::per_second(42)), 42);
```

---

## Types

### `Decision`

```rust
#[non_exhaustive]
pub enum Decision {
    Allowed,
    Denied { retry_after: Duration },
}

pub const fn is_allowed(&self) -> bool
pub const fn is_denied(&self) -> bool
pub const fn retry_after(&self) -> Option<Duration>
```

The outcome of [`acquire`](#bucketacquire). `Allowed` means the tokens were
granted and deducted. `Denied` means the request was refused; `retry_after` is
the minimum wait until the same request would succeed, or `Duration::MAX` if the
request asked for more than the capacity (it can never succeed).

`#[non_exhaustive]`: match with a wildcard arm, or use the helper methods.

**Examples**

```rust
use better_bucket::Decision;
use std::time::Duration;

assert!(Decision::Allowed.is_allowed());
assert_eq!(Decision::Allowed.retry_after(), None);

let denied = Decision::Denied { retry_after: Duration::from_millis(250) };
assert!(denied.is_denied());
assert_eq!(denied.retry_after(), Some(Duration::from_millis(250)));
```

### `BucketError`

```rust
#[non_exhaustive]
pub enum BucketError {
    ZeroCapacity,
    ZeroRefillAmount,
    ZeroRefillPeriod,
}
```

The error returned by [`BucketConfig::new`](#bucketconfig) when a configuration
cannot describe a working bucket. Each variant names the violated constraint.
Implements `std::error::Error`, `Display`, and `error_forge::ForgeError`
(`kind()` returns the variant name, e.g. `"ZeroCapacity"`).

`#[non_exhaustive]`: match with a wildcard arm.

**Examples**

```rust
use better_bucket::{BucketConfig, BucketError};
use std::time::Duration;

let err = BucketConfig::new(10, 0, Duration::from_secs(1), 0).unwrap_err();
assert_eq!(err, BucketError::ZeroRefillAmount);
assert_eq!(err.to_string(), "refill amount must be greater than zero");
```

Integrating with the `error-forge` stack:

```rust
use better_bucket::BucketError;
use error_forge::ForgeError;

assert_eq!(BucketError::ZeroCapacity.kind(), "ZeroCapacity");
assert!(!BucketError::ZeroCapacity.is_retryable());
```

---

## Crate metadata

### `VERSION`

```rust
pub const VERSION: &str;
```

The version of the linked `better-bucket` crate, captured from `Cargo.toml` at
compile time. A `&'static str` in `MAJOR.MINOR.PATCH` form, always non-empty.
Available in every build configuration, including bare `no_std`.

**Examples**

```rust
println!("better-bucket {}", better_bucket::VERSION);

let version = better_bucket::VERSION;
assert_eq!(version.split('.').count(), 3); // major.minor.patch
```

---

## Feature flags

| Feature | Default | Description |
|---------|---------|-------------|
| `std`   | yes     | Standard library. Off → `no_std`. |
| `clock` | yes     | Pluggable [`clock-lib`](https://crates.io/crates/clock-lib) time source + mockable clock for tests. **Implies `std`** (clock-lib's `Clock` is std-gated), and gates the entire `Bucket` surface. |

A bare `no_std` build (`default-features = false`) currently exposes only
[`VERSION`](#version). The `Bucket` surface requires the `clock` feature; the
`no_std`-capable, caller-driven core lands with the lock-free rewrite in `0.3`.

---

<sub>Copyright &copy; 2026 <strong>James Gober</strong>. All rights reserved.</sub>
