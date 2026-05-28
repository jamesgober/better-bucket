# better-bucket — Stability

`better-bucket` is `1.0`. This document is the SemVer contract: what is frozen,
what is promised, and what is explicitly not.

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

### `Bucket<C: Clock = SystemClock>`

`per_second`, `per_duration`, `from_config`, `builder`, `with_clock`, `acquire`,
`try_acquire`, `available`, `capacity`, `config`, `reset`. Implements `Debug`,
`Send`, and `Sync` (the latter two whenever `C` is).

### `BucketBuilder`

`new`, `capacity`, `refill`, `initial`, `build`. Implements `Debug`, `Clone`,
`Default`.

### `BucketConfig`

`new`, and the `const` accessors `capacity`, `refill_amount`, `refill_period`,
`initial`. Implements `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`.

### `Decision`

`#[non_exhaustive]` enum: `Allowed`, `Denied { retry_after: Duration }`. Methods
`is_allowed`, `is_denied`, `retry_after`. Implements `Debug`, `Clone`, `Copy`,
`PartialEq`, `Eq`. Marked `#[must_use]`.

### `BucketError`

`#[non_exhaustive]` enum: `ZeroCapacity`, `ZeroRefillAmount`, `ZeroRefillPeriod`.
Implements `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `Display`,
`std::error::Error`, and `error_forge::ForgeError`.

### `TokenBucket` trait

`acquire`, `try_acquire`, `available`, `capacity`. Object-safe. Implemented for
every `Bucket<C>`.

### `VERSION`

`pub const VERSION: &str`.

### Feature flags

- `std` (default) — standard library.
- `clock` (default) — the `clock-lib` time source; implies `std` and gates the
  `Bucket` surface.
- `comparison` — **benchmark-only**; pulls in `governor` for the comparison
  bench. Not part of the supported runtime surface; it may change or be removed
  without a major bump.

New flags may be added in minor releases; the runtime flags above will not be
renamed or removed without a `2.0`.

## Behavioural contracts

These are promised and tested; they hold within the `1.x` line.

- **Never over-grants.** Across any concurrent interleaving, total tokens granted
  never exceed the initial fill plus accrued refill; the live token count stays
  within `[0, capacity]`. Proven by `loom`, a stress test, and `proptest`.
- **No panics.** No public operation panics on any input; the acquire path is
  infallible (returns a `Decision`, never a `Result`).
- **Allocation-free acquire.** `acquire` / `try_acquire` / `available` do not
  allocate.
- **Honest `retry_after`.** When a request is denied, waiting the reported
  `retry_after` is sufficient for the same request to succeed; `Duration::MAX`
  means it can never succeed (it exceeds the capacity).
- **Degenerate Tier-1 construction** (`per_second(0)`, `per_duration(0, _)`, or a
  zero period) yields a bucket that grants nothing — well-defined, never a panic.
- **`reset`** refills to full and re-bases the refill clock to now.
- **Construction validation.** `BucketConfig::new` rejects zero capacity, zero
  refill amount, and zero refill period with a `BucketError`.

## Representation limits

Documented and stable for `1.x`:

- Capacity is effectively capped at **~4.29 million tokens** (`u32::MAX`
  millitokens); larger configured values clamp.
- `retry_after` is reported at **millisecond** resolution.
- The internal time field wraps every ~49.7 days and is handled with
  `wrapping_sub`, so an actively-used bucket refills correctly indefinitely.

## Explicitly NOT promised

- The exact text of `Display` / `Debug` output.
- Internal performance characteristics (timings may improve or shift).
- The transitive dependency tree.
- `#[doc(hidden)]` items and `#[cfg(test)]` modules.
- The behaviour of the benchmark-only `comparison` feature.

---

<sub>Copyright &copy; 2026 <strong>James Gober</strong>. All rights reserved.</sub>
