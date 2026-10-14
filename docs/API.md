# better-bucket — API Reference

> Complete reference for every public item in `better-bucket`, with examples.
> Format mirrors the portfolio standard ([metrics-lib API.md](https://github.com/jamesgober/metrics-lib/blob/main/docs/API.md)).
>
> `try_acquire` is a single `compare_exchange_weak` on one atomic word —
> allocation-free, cache-line aligned, division-free on the grant path.
> The surface below is the complete, stable public API. Items marked *(1.1)*
> were added in `1.1.0`.

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
  - [`Bucket::time_until`](#buckettime_until) *(1.1)*
  - [`Bucket::refund`](#bucketrefund) *(1.1)*
- [Tier 2 — the configured path](#tier-2--the-configured-path)
  - [`BucketConfig`](#bucketconfig)
  - [`Bucket::from_config`](#bucketfrom_config)
  - [`Bucket::with_config_and_clock`](#bucketwith_config_and_clock) *(1.1)*
  - [`Bucket::with_clock`](#bucketwith_clock)
  - [`Bucket::config`](#bucketconfig-method)
  - [`Bucket::clock`](#bucketclock) *(1.1)*
  - [`Bucket::builder` / `BucketBuilder`](#bucketbuilder)
- [Tier 3 — the power path](#tier-3--the-power-path)
  - [Caller-supplied time: the `*_at` methods](#caller-supplied-time-the-_at-methods) *(1.1)*
  - [`Bucket::acquire_up_to`](#bucketacquire_up_to) *(1.1)*
  - [`Bucket::reserve`](#bucketreserve) *(1.1)*
  - [`Bucket::metrics`](#bucketmetrics) *(1.1)*
  - [`TokenBucket` trait](#tokenbucket-trait)
- [The `no_std` core: `BucketCore`](#the-no_std-core-bucketcore) *(1.1)*
- [Types](#types)
  - [`Decision`](#decision)
  - [`BucketError`](#bucketerror)
  - [`BucketMetrics`](#bucketmetrics-type) *(1.1)*
- [Crate metadata](#crate-metadata)
  - [`VERSION`](#version)
- [Feature flags](#feature-flags)

---

## Installation

```toml
[dependencies]
better-bucket = "1.1"
```

`no_std` build — [`BucketCore`](#the-no_std-core-bucketcore) with
caller-supplied time, plus `BucketConfig`, `BucketBuilder::build_config`,
`Decision`, and `BucketError`. The clock-driven `Bucket` needs the default
`clock` feature, which implies `std`:

```toml
[dependencies]
better-bucket = { version = "1.1", default-features = false }

# Targets without native 64-bit atomics (e.g. Cortex-M):
better-bucket = { version = "1.1", default-features = false, features = ["portable-atomic"] }
```

MSRV is **1.85** (Rust 2024 edition).

### Limits

State is one `AtomicU64` holding the instant the bucket would have been empty,
in exact token units (see [`DESIGN.md`](DESIGN.md)). Consequences:

- Capacity is exact for every `u32`; `capacity()` always equals
  `config().capacity()`.
- Time is tracked in nanoseconds and **never wraps**. The bucket's clock
  saturates after at least ~438 years below 10^9 tokens/second (~100 years
  above). Past that it is frozen: the bucket grants at most what it still holds
  and then denies (a charge past the end of the range fails closed). A
  caller-supplied reading past the range — `Duration::MAX`, say — is that frozen
  instant, so validate untrusted timestamps before passing them in.
- `retry_after` and `time_until` are reported at **millisecond** resolution,
  rounded up, so waiting the reported time is always enough.
- For every rate up to ~4.3 × 10^9 tokens/second (every `u32` amount per
  second) every grant is exact; faster buckets, which need a sub-second period,
  count in multi-token units and round each grant up to a whole unit.
- The only other rounding: a token may be granted a few nanoseconds before it is exactly due (or up to 2^-28 of a token early, for a bucket whose time to fill exceeds ~36 years) (see [`DESIGN.md`](DESIGN.md#units-exact-tokens-a-converted-clock)).

---

## Overview

`better-bucket` exposes a single token bucket behind a small surface:

- **Tier 1** — a constructor (`per_second` / `per_duration`) plus `try_acquire`.
  The 80% case, one line each.
- **Tier 2** — a validated [`BucketConfig`] for full control, plus clock
  injection via [`with_clock`](#bucketwith_clock).
- **Tier 3** — caller-supplied time (`*_at`), partial grants, reservations,
  metrics, and the [`TokenBucket`](#tokenbucket-trait) trait, the abstraction a
  consumer (e.g. `rate-net`) codes against.
- **`no_std`** — [`BucketCore`](#the-no_std-core-bucketcore), the same engine with
  the caller supplying the time.

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

```rust,ignore
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

```rust,ignore
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

```rust,ignore
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

```rust,ignore
pub fn acquire(&self, n: u32) -> Decision
```

Attempts to take `n` tokens, returning the full [`Decision`](#decision). On
success the tokens are deducted and the result is `Decision::Allowed`. On
failure the bucket is untouched and the result is `Decision::Denied`, carrying
the minimum `retry_after` until the same request would succeed.

**Parameters**

- `n` — tokens to take.

**Returns** a [`Decision`](#decision). A denial's `retry_after` is the wait,
rounded up to the millisecond, until the same request would succeed. Requesting
more than the capacity returns `Denied { retry_after: Duration::MAX }` (it can
never succeed).

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

```rust,ignore
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

Observing refill deterministically with an injected clock:

```rust
use better_bucket::Bucket;
use clock_lib::ManualClock;
use std::sync::Arc;
use std::time::Duration;

let clock = Arc::new(ManualClock::new());
let bucket = Bucket::per_second(100).with_clock(Arc::clone(&clock));

assert!(bucket.try_acquire(100));         // drain
assert_eq!(bucket.available(), 0);
clock.advance(Duration::from_millis(250)); // a quarter second
assert_eq!(bucket.available(), 25);        // 25% of the per-second rate
```

### `Bucket::capacity`

```rust,ignore
pub const fn capacity(&self) -> u32
```

Returns the bucket's capacity — its burst ceiling, the maximum tokens it can
hold.

**Examples**

```rust
use better_bucket::Bucket;

assert_eq!(Bucket::per_second(64).capacity(), 64);
```

The capacity is the largest burst grantable at once:

```rust
use better_bucket::Bucket;

let bucket = Bucket::per_second(50);
assert!(bucket.try_acquire(bucket.capacity())); // a full-capacity burst
assert!(!bucket.try_acquire(1));                 // now empty
```

### `Bucket::reset`

```rust,ignore
pub fn reset(&self)
```

Refills the bucket to full and marks it current as of now. Use it to discard
accumulated debt (including outstanding [reservations](#bucketreserve)) and
grant a fresh burst — for example at the start of a new billing window. Long
uptime needs no special handling: the clock never wraps.

**Examples**

```rust
use better_bucket::Bucket;

let bucket = Bucket::per_second(4);
assert!(bucket.try_acquire(4));
assert_eq!(bucket.available(), 0);
bucket.reset();
assert_eq!(bucket.available(), 4);
```

Clearing a backlog so the next window starts clean:

```rust
use better_bucket::Bucket;

let quota = Bucket::per_second(1_000);
assert!(quota.try_acquire(800)); // 800 spent this window
// New billing window begins — forgive the spend and start full.
quota.reset();
assert_eq!(quota.available(), 1_000);
```

### `Bucket::time_until`

```rust,ignore
pub fn time_until(&self, n: u32) -> Option<Duration>   // (1.1)
```

Returns how long until `n` tokens could be acquired, **without taking any** —
the `Retry-After` value without spending a token to learn it.

**Returns** `Some(Duration::ZERO)` if `n` tokens are available now; `Some(wait)`
with the same honest, millisecond-rounded-up hint a denial would carry; `None`
if the request can never succeed (`n` exceeds the capacity). A snapshot, like
[`available`](#bucketavailable).

**Examples**

```rust
use better_bucket::Bucket;
use clock_lib::ManualClock;
use std::sync::Arc;
use std::time::Duration;

let clock = Arc::new(ManualClock::new());
let bucket = Bucket::per_second(10).with_clock(Arc::clone(&clock));
assert!(bucket.try_acquire(10));
assert_eq!(bucket.time_until(5), Some(Duration::from_millis(500)));
assert_eq!(bucket.available(), 0); // nothing was taken
assert_eq!(bucket.time_until(11), None);
```

Populating a `Retry-After` header before doing any work:

```rust
use better_bucket::Bucket;

let limiter = Bucket::per_second(100);
match limiter.time_until(1) {
    Some(wait) if wait.is_zero() => { /* admit: acquire and serve */ }
    Some(wait) => { let _header_secs = wait.as_secs_f64().ceil() as u64; }
    None => { /* request can never fit this limiter */ }
}
```

### `Bucket::refund`

```rust,ignore
pub fn refund(&self, n: u32)   // (1.1)
```

Returns `n` previously acquired tokens to the bucket, for work that was cancelled
or failed before it used its budget. The balance never exceeds the capacity, and
a refund also pays down an outstanding [reservation](#bucketreserve). Only refund
what you took: refunding tokens that were never acquired lets the bucket grant
beyond its rate.

**Examples**

```rust
use better_bucket::Bucket;

let bucket = Bucket::per_second(10);
assert!(bucket.try_acquire(10));
bucket.refund(4); // the upstream call for 4 of them failed
assert!(bucket.available() >= 4);
```

---

## Tier 2 — the configured path

### `BucketConfig`

```rust,ignore
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

Every capacity `new` accepts is held exactly, and every rate refills on time;
only rates above ~4.3 × 10^9 tokens/second round grants up to multi-token
units. `new` is
a `const fn` *(1.1)*, so a configuration can be validated at compile time.
`BucketConfig` implements `Hash` *(1.1)*, and with the `serde` feature
`Serialize` / `Deserialize` *(1.1)* — deserialization runs the same validation as
`new`, rejects unknown fields, and uses serde's standard `Duration` form for
`refill_period` (`{ "secs": 1, "nanos": 0 }`).

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

Validated at compile time:

```rust
use better_bucket::BucketConfig;
use std::time::Duration;

const LIMIT: BucketConfig = match BucketConfig::new(10, 10, Duration::from_secs(1), 10) {
    Ok(config) => config,
    Err(_) => panic!("invalid bucket configuration"),
};
```

### `Bucket::from_config`

```rust,ignore
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

A large burst ceiling that refills slowly, starting full:

```rust
use better_bucket::{Bucket, BucketConfig};
use std::time::Duration;

// Hold up to 10_000, but only top up 10/second.
let config = BucketConfig::new(10_000, 10, Duration::from_secs(1), 10_000)?;
let bucket = Bucket::from_config(config);
assert!(bucket.try_acquire(10_000)); // the whole burst, at once
assert!(!bucket.try_acquire(1));     // and now it refills slowly
# Ok::<(), better_bucket::BucketError>(())
```

### `Bucket::with_config_and_clock`

```rust,ignore
pub fn with_config_and_clock(config: BucketConfig, clock: C) -> Bucket<C>   // (1.1)
```

Creates a bucket from a config and a clock in one step, holding its initial
tokens as of the clock's current reading. One build and one clock read, where
`from_config(config).with_clock(clock)` builds a `SystemClock` bucket first and
then rebuilds it — the constructor to use for per-key buckets with a shared
injected clock.

**Examples**

```rust
use better_bucket::{Bucket, BucketConfig};
use clock_lib::ManualClock;
use std::sync::Arc;
use std::time::Duration;

let clock = Arc::new(ManualClock::new());
let quota = BucketConfig::new(100, 10, Duration::from_secs(1), 0)?;
let alice = Bucket::with_config_and_clock(quota, Arc::clone(&clock));
let bob = Bucket::with_config_and_clock(quota, Arc::clone(&clock));

clock.advance(Duration::from_secs(1));
assert_eq!(alice.available(), 10);
assert_eq!(bob.available(), 10);
# Ok::<(), better_bucket::BucketError>(())
```

### `Bucket::with_clock`

```rust,ignore
pub fn with_clock<C2: Clock>(self, clock: C2) -> Bucket<C2>
```

Replaces the bucket's time source, resetting it to its initial fill anchored at
the new clock's current reading; tokens already taken from `self` are
forgotten. This is the clock-injection seam for chaining onto the Tier-1
constructors, chiefly in tests, where a
[`ManualClock`](https://docs.rs/clock-lib) makes refill deterministic with no
`sleep`. Prefer [`with_config_and_clock`](#bucketwith_config_and_clock) when
building from a config.

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

Chaining onto a configured bucket, so a test drives a non-default quota:

```rust
use better_bucket::{Bucket, BucketConfig};
use clock_lib::ManualClock;
use std::sync::Arc;
use std::time::Duration;

let clock = Arc::new(ManualClock::new());
let config = BucketConfig::new(100, 10, Duration::from_secs(1), 0)?;
let bucket = Bucket::from_config(config).with_clock(Arc::clone(&clock));

assert_eq!(bucket.available(), 0);   // started empty
clock.advance(Duration::from_secs(1));
assert_eq!(bucket.available(), 10);  // one second of refill
# Ok::<(), better_bucket::BucketError>(())
```

### `Bucket::config` <a id="bucketconfig-method"></a>

```rust,ignore
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

Reading back every field of a configured bucket:

```rust
use better_bucket::Bucket;
use std::time::Duration;

let config = Bucket::builder()
    .capacity(500)
    .refill(100, Duration::from_secs(1))
    .initial(0)
    .build()?
    .config();

assert_eq!(config.capacity(), 500);
assert_eq!(config.refill_amount(), 100);
assert_eq!(config.initial(), 0);
# Ok::<(), better_bucket::BucketError>(())
```

### `Bucket::clock`

```rust,ignore
pub const fn clock(&self) -> &C   // (1.1)
```

Returns the bucket's clock, for reading the time once and passing it to the
[`*_at` methods](#caller-supplied-time-the-_at-methods).

```rust
use better_bucket::Bucket;
use clock_lib::Clock;

let bucket = Bucket::per_second(5);
let now = bucket.clock().now();
assert!(bucket.try_acquire_at(1, now));
```

### `Bucket::builder`

```rust,ignore
pub const fn builder() -> BucketBuilder

// BucketBuilder
pub const fn new() -> BucketBuilder
pub const fn capacity(self, capacity: u32) -> BucketBuilder
pub const fn refill(self, amount: u32, period: Duration) -> BucketBuilder
pub const fn initial(self, initial: u32) -> BucketBuilder
pub fn build(self) -> Result<Bucket<SystemClock>, BucketError>
pub fn build_with_clock<C: Clock>(self, clock: C) -> Result<Bucket<C>, BucketError>  // (1.1)
pub const fn build_config(self) -> Result<BucketConfig, BucketError>                 // (1.1)
```

The fluent Tier-2 entry point. Set the capacity (burst ceiling), the refill rate,
and optionally the initial fill, then `build`. Anything unset keeps its default;
`build` validates through [`BucketConfig::new`](#bucketconfig), so an unworkable
combination is rejected. `initial` defaults to the capacity (the bucket starts
full). `build` produces a `SystemClock` bucket; `build_with_clock` takes any
clock; `build_config` stops at the validated config and is available without
the `clock` feature (pair it with [`BucketCore`](#the-no_std-core-bucketcore)).

**`build` errors:** the same as [`BucketConfig::new`](#bucketconfig) —
`ZeroCapacity`, `ZeroRefillAmount`, `ZeroRefillPeriod`. A freshly created builder
fails this way until a capacity and refill rate are set.

**Examples**

```rust
use better_bucket::Bucket;
use std::time::Duration;

// Large burst ceiling, slow refill, starting empty.
let bucket = Bucket::builder()
    .capacity(1000)
    .refill(50, Duration::from_secs(1))
    .initial(0)
    .build()?;
assert_eq!(bucket.capacity(), 1000);
assert_eq!(bucket.available(), 0);
# Ok::<(), better_bucket::BucketError>(())
```

An unconfigured builder is rejected:

```rust
use better_bucket::{Bucket, BucketError};

assert_eq!(Bucket::builder().build().unwrap_err(), BucketError::ZeroCapacity);
```

With an injected clock, or stopping at the config:

```rust
use better_bucket::{Bucket, BucketBuilder};
use clock_lib::ManualClock;
use std::sync::Arc;
use std::time::Duration;

let clock = Arc::new(ManualClock::new());
let bucket = Bucket::builder()
    .capacity(10)
    .refill(1, Duration::from_secs(1))
    .initial(0)
    .build_with_clock(Arc::clone(&clock))?;
clock.advance(Duration::from_secs(3));
assert_eq!(bucket.available(), 3);

let config = BucketBuilder::new().capacity(5).refill(5, Duration::from_secs(1)).build_config()?;
assert_eq!(config.initial(), 5);
# Ok::<(), better_bucket::BucketError>(())
```

---

## Tier 3 — the power path

### Caller-supplied time: the `*_at` methods

```rust,ignore
// (1.1)
pub fn acquire_at(&self, n: u32, now: Monotonic) -> Decision
pub fn try_acquire_at(&self, n: u32, now: Monotonic) -> bool
pub fn available_at(&self, now: Monotonic) -> u32
pub fn time_until_at(&self, n: u32, now: Monotonic) -> Option<Duration>
pub fn acquire_up_to_at(&self, n: u32, now: Monotonic) -> u32
pub fn reserve_at(&self, n: u32, max_wait: Duration, now: Monotonic) -> Option<Duration>
pub fn reset_at(&self, now: Monotonic)
```

Every method that reads the clock has a twin that takes the reading from the
caller. Read the clock once and decide for several buckets (a keyed limiter
checking a per-user and a per-route bucket), or decide at a known instant.
`now` should come from the bucket's clock or one that agrees with it. A reading
earlier than the bucket's construction counts as the construction instant; a
reading earlier than the last grant simply sees fewer tokens — it can never
cause an extra grant.

```rust
use better_bucket::Bucket;
use clock_lib::{Clock, ManualClock};
use std::sync::Arc;

let clock = Arc::new(ManualClock::new());
let per_user = Bucket::per_second(10).with_clock(Arc::clone(&clock));
let per_route = Bucket::per_second(1_000).with_clock(Arc::clone(&clock));

let now = clock.now(); // one read
let admitted = per_user.try_acquire_at(1, now) && per_route.try_acquire_at(1, now);
assert!(admitted);
```

### `Bucket::acquire_up_to`

```rust,ignore
pub fn acquire_up_to(&self, n: u32) -> u32   // (1.1)
```

Takes as many of `n` tokens as are available right now (possibly zero) and
returns how many it took — the partial grant for byte-oriented throttling. Never
takes more than the capacity.

```rust
use better_bucket::Bucket;

let bandwidth = Bucket::per_second(64 * 1024); // bytes per second
let chunk = 100 * 1024;
let allowed = bandwidth.acquire_up_to(chunk);
assert_eq!(allowed, 64 * 1024); // write this much now, the rest later
```

### `Bucket::reserve`

```rust,ignore
pub fn reserve(&self, n: u32, max_wait: Duration) -> Option<Duration>   // (1.1)
```

Reserves `n` tokens now, even if they have not accrued yet, as long as the wait
until they are covered is at most `max_wait`. On success the tokens are taken
immediately and the returned duration is how long to wait before acting on them
(zero if they were already there). Later callers queue behind the reservation,
so a large request cannot be starved by a stream of small ones — a plain token
bucket has no queue, so without this a big `acquire` can be starved
indefinitely. Returns `None`, reserving nothing, if `n` exceeds the capacity or
the wait would exceed `max_wait`. The bucket never sleeps; pass the wait to
`std::thread::sleep` or an async timer.

```rust
use better_bucket::Bucket;
use clock_lib::ManualClock;
use std::sync::Arc;
use std::time::Duration;

let clock = Arc::new(ManualClock::new());
let bucket = Bucket::per_second(10).with_clock(Arc::clone(&clock));
assert!(bucket.try_acquire(10)); // empty

assert_eq!(bucket.reserve(10, Duration::from_secs(5)), Some(Duration::from_secs(1)));
assert!(!bucket.try_acquire(1)); // queued behind the reservation
assert_eq!(bucket.reserve(10, Duration::from_millis(100)), None); // too long a wait
```

### `Bucket::metrics`

```rust,ignore
#[cfg(feature = "metrics")]
pub fn metrics(&self) -> BucketMetrics   // (1.1)
```

A snapshot of the bucket's decision counters; see
[`BucketMetrics`](#bucketmetrics-type). Only with the `metrics` feature, which is
off by default and costs nothing when off.

### `TokenBucket` trait

```rust,ignore
pub trait TokenBucket {
    fn acquire(&self, n: u32) -> Decision;
    fn try_acquire(&self, n: u32) -> bool;
    fn available(&self) -> u32;
    fn capacity(&self) -> u32;
    fn time_until(&self, n: u32) -> Option<Duration> { /* provided */ }  // (1.1)
    fn reset(&self) { /* provided: no-op */ }                            // (1.1)
}
```

The abstraction a consumer codes against, so it can hold a bucket without naming
its concrete clock type. Implemented for every `Bucket<C>`. The methods mirror
the inherent methods of [`Bucket`] exactly; see those for each contract. The
trait is object-safe — `&dyn TokenBucket` works.

`time_until` and `reset` were added in `1.1` with provided bodies, so an
implementation written against `1.0` keeps compiling. `Bucket` overrides both.
The provided `time_until` cannot see a refill rate, so it answers
`Some(Duration::ZERO)` when `available() >= n` and `None` ("no prediction")
otherwise; the provided `reset` does nothing. Other implementations should
override them.

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

## The `no_std` core: `BucketCore`

```rust,ignore
// (1.1) — available without `std`
pub fn new(config: BucketConfig, now: Duration) -> BucketCore
pub fn acquire_at(&self, n: u32, now: Duration) -> Decision
pub fn try_acquire_at(&self, n: u32, now: Duration) -> bool
pub fn acquire_up_to_at(&self, n: u32, now: Duration) -> u32
pub fn available_at(&self, now: Duration) -> u32
pub fn time_until_at(&self, n: u32, now: Duration) -> Option<Duration>
pub fn reserve_at(&self, n: u32, max_wait: Duration, now: Duration) -> Option<Duration>
pub fn refund(&self, n: u32)
pub fn reset_at(&self, now: Duration)
pub const fn capacity(&self) -> u32
pub const fn config(&self) -> BucketConfig
#[cfg(feature = "metrics")] pub fn metrics(&self) -> BucketMetrics
```

The same engine as `Bucket` (lock-free on native 64-bit atomics), without a clock. Every time-dependent
method takes `now`: the time elapsed since an epoch of your choosing (boot, a
timer peripheral's zero, a test's start), the same epoch for every call on one
bucket. The bucket holds its initial tokens as of the `now` passed to `new`, so
the epoch does not have to be the creation time. It needs only `core` and a
64-bit atomic — native on most targets, or via the `portable-atomic` feature on
targets such as Cortex-M — so it is the `no_std` path, and the fully
deterministic one. Every method has the contract of its `Bucket` counterpart.
`now` should not run backwards; if it does, the bucket sees fewer tokens until
time catches up, never more. Implements `Debug`, `Send`, `Sync`.

**Examples**

```rust
use better_bucket::{BucketConfig, BucketCore};
use core::time::Duration;

// 10 tokens/second, burst 10, starting full at t = 0.
let config = BucketConfig::new(10, 10, Duration::from_secs(1), 10)?;
let bucket = BucketCore::new(config, Duration::ZERO);

assert!(bucket.try_acquire_at(10, Duration::ZERO));
assert!(!bucket.try_acquire_at(1, Duration::ZERO));
assert_eq!(bucket.available_at(Duration::from_millis(500)), 5);
assert_eq!(bucket.time_until_at(10, Duration::from_millis(500)), Some(Duration::from_millis(500)));
# Ok::<(), better_bucket::BucketError>(())
```

A firmware loop driving it from a tick counter:

```rust
use better_bucket::{BucketConfig, BucketCore};
use core::time::Duration;

let uplink = BucketCore::new(BucketConfig::new(5, 5, Duration::from_secs(1), 5)?, Duration::ZERO);
let mut sent = 0;
for tick_ms in (0..1_000).step_by(50) {
    if uplink.try_acquire_at(1, Duration::from_millis(tick_ms)) {
        sent += 1;
    }
}
assert_eq!(sent, 9); // the burst of 5, plus one every 200 ms
# Ok::<(), better_bucket::BucketError>(())
```

---

## Types

### `Decision`

```rust,ignore
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
the wait, rounded up to the millisecond, until the same request would succeed,
or `Duration::MAX` if it can never succeed (it asked for more than the
capacity, or the wait would outlast the bucket's ~438-year clock range).
Implements `Hash` *(1.1)*. Available without `std`.

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

Matching the outcome — the typical gatekeeper shape, turning a denial's
`retry_after` into a `Retry-After` header value:

```rust
use better_bucket::{Bucket, Decision};

let limiter = Bucket::per_second(1);
assert!(limiter.try_acquire(1)); // spend the one token

match limiter.acquire(1) {
    Decision::Allowed => { /* serve the request */ }
    Decision::Denied { retry_after } => {
        let seconds = retry_after.as_secs_f64().ceil() as u64;
        assert!(seconds >= 1); // tell the client when to come back
    }
    _ => {}
}
```

### `BucketError`

```rust,ignore
#[non_exhaustive]
pub enum BucketError {
    ZeroCapacity,
    ZeroRefillAmount,
    ZeroRefillPeriod,
}
```

The error returned by [`BucketConfig::new`](#bucketconfig) when a configuration
cannot describe a working bucket. Each variant names the violated constraint.
Implements `std::error::Error` (as `core::error::Error`, so also in `no_std`),
`Display`, `Hash` *(1.1)*, and — with the `forge` feature, implied by `clock` —
`error_forge::ForgeError` (`kind()` returns the variant name, e.g.
`"ZeroCapacity"`).

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

### `BucketMetrics` <a id="bucketmetrics-type"></a>

```rust,ignore
// (1.1), `metrics` feature
#[non_exhaustive]
pub struct BucketMetrics {
    pub allowed: u64,
    pub denied: u64,
    pub tokens_granted: u64,
}
```

A snapshot of a bucket's decision counters, returned by `Bucket::metrics` and
`BucketCore::metrics`. Cumulative since construction, updated with relaxed
atomics on every decision. With the feature off the counters do not exist — no
field, no increment.

- `allowed` — calls that granted their request (a zero-token request, a
  successful reservation, and a partial grant of at least one token count).
- `denied` — calls that granted nothing for a non-zero request. A steadily
  climbing deny rate means the quota is below demand; a spike usually means a
  burst or a misbehaving client.
- `tokens_granted` — total tokens handed out. Over elapsed time it is the
  achieved throughput, which never exceeds the configured rate by more than the
  capacity.

`reset` and `refund` are not counted: they move tokens, they are not decisions.

```rust
use better_bucket::Bucket;

let bucket = Bucket::per_second(2);
let _ = bucket.try_acquire(2);
let _ = bucket.try_acquire(1);
let m = bucket.metrics();
assert_eq!((m.allowed, m.denied, m.tokens_granted), (1, 1, 2));
```

---

## Crate metadata

### `VERSION`

```rust,ignore
pub const VERSION: &str;
```

The version of the linked `better-bucket` crate, captured from `Cargo.toml` at
compile time. A `&'static str` in `MAJOR.MINOR.PATCH` form, always non-empty.
Available in every build configuration, including bare `no_std`.

**Examples**

```rust
let version = better_bucket::VERSION;
assert_eq!(version.split('.').count(), 3); // major.minor.patch
```

Reporting the linked version in a diagnostics line:

```rust
let banner = format!("rate limiter: better-bucket v{}", better_bucket::VERSION);
assert!(banner.contains("better-bucket v"));
```

---

## Feature flags

| Feature | Default | Description |
|---------|---------|-------------|
| `std`   | yes     | Standard library. Off → `no_std`: `BucketCore`, `BucketConfig`, `BucketBuilder::build_config`, `Decision`, `BucketError` remain. |
| `clock` | yes     | Pluggable [`clock-lib`](https://crates.io/crates/clock-lib) time source + mockable clock for tests. **Implies `std`** (clock-lib's `Clock` is std-gated) **and `forge`**, and gates `Bucket`, `TokenBucket`, and `BucketBuilder::build`. |
| `forge` | yes (via `clock`) | *(1.1)* `error_forge::ForgeError` for `BucketError`. |
| `serde` | no      | *(1.1)* `Serialize` / validating `Deserialize` for `BucketConfig`. |
| `metrics` | no    | *(1.1)* Decision counters (`metrics()`), zero cost when off. |
| `portable-atomic` | no | *(1.1)* [`portable-atomic`](https://crates.io/crates/portable-atomic) for the state word, for targets without native 64-bit atomics. |

`BucketCore` needs a 64-bit atomic. On a target without one (e.g.
`thumbv7em-none-eabihf`) a bare `no_std` build exposes the config, builder,
decision, and error types; add `portable-atomic` for `BucketCore`. The
benchmark-only `comparison` feature is not part of the supported surface.

---

<sub>Copyright &copy; 2026 <strong>James Gober</strong>. All rights reserved.</sub>
