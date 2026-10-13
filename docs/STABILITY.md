# better-bucket — Stability

`better-bucket` is `1.x` (currently `1.1`). This document is the SemVer
contract: what is frozen, what is promised, and what is explicitly not. Items
marked *(1.1)* were added in `1.1.0` and are frozen from that release on.

## SemVer commitment

- **Strict SemVer.** Removing, renaming, or changing the signature of anything
  in the frozen surface below requires a `2.0` release. Additive changes (new
  methods, types, trait methods with defaults, enum variants behind
  `#[non_exhaustive]`, feature flags) are minor.
- **MSRV is Rust 1.85**, frozen at `1.0`. An MSRV increase within the `1.x` line
  is announced in the CHANGELOG and treated as a minor change, not breaking.
- **Edition 2024.**

## Frozen public surface

Everything below is committed under the SemVer contract.

### `Bucket<C: Clock = SystemClock>` (`clock` feature)

`per_second`, `per_duration`, `from_config`, `builder`, `with_clock`, `acquire`,
`try_acquire`, `available`, `capacity`, `config`, `reset`.

*(1.1)* `with_config_and_clock`, `acquire_at`, `try_acquire_at`, `available_at`,
`time_until`, `time_until_at`, `acquire_up_to`, `acquire_up_to_at`, `reserve`,
`reserve_at`, `refund`, `reset_at`, `clock`, and `metrics` (`metrics` feature).

Implements `Debug`, `Send`, and `Sync` (the latter two whenever `C` is).

### `BucketCore` *(1.1)*

The caller-driven engine, available in every build including `no_std` (on
targets with native 64-bit atomics, or anywhere with the `portable-atomic`
feature, which is lock-based and not interrupt-safe where the target has no
native 64-bit atomics). `new`, `acquire_at`, `try_acquire_at`, `acquire_up_to_at`,
`available_at`, `time_until_at`, `reserve_at`, `refund`, `reset_at`,
`capacity`, `config`, and `metrics` (`metrics` feature). Implements `Debug`,
`Send`, `Sync`.

### `BucketBuilder`

`new`, `capacity`, `refill`, `initial`, `build` (`clock` feature). Implements
`Debug`, `Clone`, `Default`.

*(1.1)* `build_config` and `build_with_clock` (`clock` feature). `new`,
`capacity`, `refill`, `initial`, and `build_config` are `const fn`. The type is
available without `clock`.

### `BucketConfig`

`new`, and the `const` accessors `capacity`, `refill_amount`, `refill_period`,
`initial`. Implements `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`.

*(1.1)* `new` is a `const fn`. Implements `Hash`, and under the `serde` feature
`Serialize` and a `Deserialize` that runs the same validation as `new`.
Available without `clock`.

### `Decision`

`#[non_exhaustive]` enum: `Allowed`, `Denied { retry_after: Duration }`. Methods
`is_allowed`, `is_denied`, `retry_after`. Implements `Debug`, `Clone`, `Copy`,
`PartialEq`, `Eq`, and *(1.1)* `Hash`. Marked `#[must_use]`. *(1.1)* Available
without `clock`.

### `BucketError`

`#[non_exhaustive]` enum: `ZeroCapacity`, `ZeroRefillAmount`, `ZeroRefillPeriod`.
Implements `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, *(1.1)* `Hash`,
`Display`, `std::error::Error` (implemented as `core::error::Error`, the same
trait, in every build), and `error_forge::ForgeError` (`forge` feature, implied
by `clock`). *(1.1)* Available without `clock`.

### `BucketMetrics` *(1.1, `metrics` feature)*

`#[non_exhaustive]` struct with public `u64` fields `allowed`, `denied`,
`tokens_granted`. Implements `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`,
`Hash`, `Default`. Further counters may be added as new fields.

### `TokenBucket` trait (`clock` feature)

`acquire`, `try_acquire`, `available`, `capacity`. *(1.1)* `time_until` and
`reset`, with provided bodies so existing implementations keep compiling.
Object-safe. Implemented for every `Bucket<C>`.

### `VERSION`

`pub const VERSION: &str`.

### Feature flags

- `std` (default) — standard library. Off → `no_std`.
- `clock` (default) — the `clock-lib` time source; implies `std` and `forge`,
  and gates `Bucket`, `TokenBucket`, and `BucketBuilder::build`.
- `forge` *(1.1)* — `error_forge::ForgeError` for `BucketError`. Implied by
  `clock`, so every `1.0` feature combination keeps the impl; making `clock`
  independent of `forge` would be a `2.0` change.
- `serde` *(1.1)* — `serde` support for `BucketConfig`.
- `metrics` *(1.1)* — decision counters.
- `portable-atomic` *(1.1)* — `portable-atomic` for the state word, for targets
  without native 64-bit atomics.
- `comparison` — **benchmark-only**; pulls in `governor` for the comparison
  bench. Not part of the supported runtime surface; it may change or be removed
  without a major bump.

New flags may be added in minor releases; the runtime flags above will not be
renamed or removed without a `2.0`.

## Behavioural contracts

These are promised and tested; they hold within the `1.x` line.

- **Never over-grants.** Under any interleaving of concurrent callers, the
  tokens granted over any window never exceed the capacity plus the refill
  accrued over that window — to within the bucket's resolution: a token may be granted a few nanoseconds before it is exactly due (or up to 2^-28 of a token early, for a bucket whose time to fill exceeds ~36 years) — and the live token count stays within
  `[0, capacity]`. A stale or backward clock reading, or one past the end of
  the clock's range, can only cause a denial. `reset` and `refund` add tokens
  deliberately and are outside this bound, and tokens taken by `reserve` count
  at the end of their returned wait: the tokens reserved by any instant never
  exceed the capacity plus the refill accrued by the time their waits end.
  Checked by `loom` with an advancing clock, a real-clock contention test, and
  `proptest`. (`1.0.0` stated this but did not hold it under real-clock
  contention; fixed in `1.1.0`.)
- **Exact token accounting.** For every rate up to ~4.3 × 10^9 tokens/second
  (every `u32` amount per second), grants,
  refunds, and the capacity are exact: with no refill, every token is conserved.
- **No panics.** No public operation panics on any input; the acquire path is
  infallible (returns a `Decision`, never a `Result`).
- **Allocation-free acquire.** No per-call operation allocates.
- **Honest `retry_after`.** When a request is denied, waiting the reported
  `retry_after` is sufficient for the same request to succeed (absent other
  callers). `Duration::MAX` means it can never succeed: it exceeds the capacity,
  or the wait would outlast the clock's range. *(1.1)* `time_until` returns the
  same wait without taking anything, `Some(Duration::ZERO)` when the request
  would succeed now, and `None` for "never".
- **Degenerate Tier-1 construction** (`per_second(0)`, `per_duration(0, _)`, or a
  zero period) yields a bucket that grants nothing — well-defined, never a panic.
- **`reset`** refills to full as of now, discarding any reservation debt.
- *(1.1)* **`capacity() == config().capacity()`**, always.
- **Construction validation.** `BucketConfig::new` rejects zero capacity, zero
  refill amount, and zero refill period with a `BucketError`. Every capacity
  it accepts is held exactly and every rate refills on time; only rates above
  ~4.3 × 10^9 tokens/second round grants up to multi-token units.

## Representation limits

Documented and stable for `1.x`:

- Capacity is exact for every `u32` *(1.1; `1.0` clamped it at ~4.29 million
  tokens)*.
- `retry_after` and `time_until` are reported at **millisecond** resolution,
  rounded up.
- Time is tracked in nanoseconds and never wraps *(1.1; `1.0` wrapped every
  ~49.7 days)*. The bucket's clock saturates after at least ~438 years below
  10^9 tokens/second (~100 years above), after which it is frozen: the bucket
  grants at most what it still holds and then denies.
- Rates above ~4.3 × 10^9 tokens/second count tokens in multi-token units and
  round each grant up to a whole unit.

## Explicitly NOT promised

- The exact text of `Display` / `Debug` output.
- Internal performance characteristics (timings may improve or shift), and the
  size of `Bucket` / `BucketCore` (currently one 64-byte cache line on 64-bit
  targets without `metrics`).
- The exact `retry_after` value beyond "an honest upper bound at millisecond
  resolution": `1.1` reports some waits a millisecond shorter than `1.0` did,
  because `1.0`'s rate truncation is gone.
- The transitive dependency tree.
- `#[doc(hidden)]` items and `#[cfg(test)]` modules.
- The behaviour of the benchmark-only `comparison` feature.

---

<sub>Copyright &copy; 2026 <strong>James Gober</strong>. All rights reserved.</sub>
