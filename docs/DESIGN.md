# better-bucket — Design

How the bucket works inside, and why it is built the way it is. For the public
API see [`API.md`](API.md); for numbers see [`BENCHMARKS.md`](BENCHMARKS.md).

## The one invariant

Everything here serves a single safety contract: **the bucket never
over-grants.** Across any concurrent interleaving, the total tokens handed out
never exceed the initial fill plus what the configured rate legitimately accrues
over the elapsed time, and the live token count always stays within
`[0, capacity]`. Every design choice below is in service of making that true and
cheap.

## State: one atomic word

All mutable state is a single `AtomicU64`, split in two:

```
 63                              32 31                               0
┌─────────────────────────────────┬──────────────────────────────────┐
│  tokens, in millitokens (u32)    │  milliseconds since construction  │
└─────────────────────────────────┴──────────────────────────────────┘
```

- **Tokens are stored in millitokens** (thousandths of a token). A token bucket
  must accrue *fractions* of a token between calls — at 100 tokens/second, 5 ms
  is half a token — and dropping that fraction would make the bucket refill
  slower than configured. Millitokens carry it. The cost is a capacity ceiling:
  `u32::MAX` millitokens is about **4.29 million tokens**, which is an enormous
  burst for rate limiting; larger configured capacities clamp to it.
- **Time is milliseconds since construction.** Relative, not absolute, so it
  fits in 32 bits.

Keeping both fields in one word is what makes the acquire path a single
compare-and-swap. A design with two atomics (tokens and time separately) cannot
update them together atomically, which reopens exactly the lost-update and
over-grant races this crate exists to prevent.

The whole struct is `#[repr(align(64))]` so independent buckets never share a
cache line. Only the `state` word is ever written; the rate and capacity are
immutable after construction, so they cannot cause false sharing.

## Acquire: a CAS loop with lazy refill

`try_acquire(n)` reads the clock once, then:

1. Load the packed word; unpack `(tokens, last_ms)`.
2. Compute the lazily-refilled token count as of now (saturating, clamped to
   capacity).
3. If `n` tokens are available, `compare_exchange_weak` the new
   `(tokens − n, now)` into place; on success, allow. On CAS failure (another
   thread won the race, or a spurious weak failure), retry from step 1.
4. If not enough tokens, return a denial — **without writing**, so a denied
   request never contends with a granting one.

There is no lock, no allocation, and no syscall on the success path beyond the
monotonic clock read.

### Memory ordering

The loads and the CAS use `Relaxed`. The bucket publishes no data alongside the
counter — the only shared state is the one word — so all that is required is the
read-modify-write *atomicity* the no-over-grant contract depends on, which a
`Relaxed` CAS provides (a single atomic location always has a total modification
order). Stronger ordering would buy nothing and cost on weakly-ordered targets.
`loom` checks this directly.

## Refill: division-free fixed point

The refilled token count is

```
tokens + elapsed_ms × (millitokens per millisecond)   (clamped to capacity)
```

Computing `millitokens-per-ms` as `refill_amount × 1e9 / period_nanos` on every
acquire would put a 128-bit division on the hot path. Instead the rate is
precomputed once at construction as a **`Q22` fixed-point** multiplier
(millitokens-per-ms scaled by `2^22`), so the hot path is a multiply and a
shift. Deriving the rate from the period in *nanoseconds* keeps sub-millisecond
periods accurate even though the time field ticks in milliseconds. When no whole
millisecond has elapsed since the last refill — the common case under bursty
load — the computation is skipped entirely.

The denial's `retry_after` is the inverse: the time for the shortfall to accrue,
computed with a **ceiling** so the hint never under-promises. Ceiling-for-the-wait
and floor-for-the-refill must agree, or a caller that waited exactly as long as
told could still be denied; the `retry_after_is_an_honest_lower_bound` property
test proves they do.

## Time: wrapping, not saturating

The 32-bit millisecond field wraps every ~49.7 days. Elapsed time is computed
with `wrapping_sub`, which recovers the true interval for any gap shorter than
that window — i.e. for any bucket touched at least once per ~49.7 days, which is
every real limiter. So an actively-used bucket refills correctly **indefinitely**.

An earlier design *saturated* the field at `u32::MAX`, which made a long-running
bucket stop refilling after ~49.7 days — a latent outage for a primitive meant
to run for months. Wrapping removes it. The only residual edge is a bucket left
*fully idle* for longer than the window, which may under-refill once on its next
use: a safe, conservative, self-correcting outcome (it never over-grants, only
under-grants briefly).

## Overflow safety

Every refill and capacity computation is saturating or checked. A hostile
request count, an enormous elapsed gap, or a near-infinite rate cannot wrap the
counter or over-fill the bucket: intermediate products use `u128`, accrual
saturates, and the result is clamped to capacity. Construction rejects
nonsensical configurations (zero capacity, amount, or period) up front with a
`BucketError`, rather than producing a bucket that misbehaves later. The Tier-1
constructors stay infallible by interpreting a degenerate argument as a bucket
that grants nothing.

## How the contract is defended

- **`loom`** exhaustively explores the CAS interleavings of concurrent
  `try_acquire` calls and asserts the bucket grants exactly the available tokens
  — no over-grant, no lost token. Under `--cfg loom` the `state` word is a
  `loom` atomic so the model checker sees it.
- **A multi-thread stress test** hammers one bucket from eight threads with no
  refill and asserts exactly the capacity is granted.
- **An allocation audit** runs the acquire path under a per-thread counting
  allocator and asserts zero allocations.
- **`proptest`** asserts, over a wide input space, that tokens stay within
  `[0, capacity]`, grants never exceed the initial fill plus accrued refill, and
  the `retry_after` hint is honest.
- **An adversarial/edge suite** covers `u32::MAX` requests, `u32::MAX` capacity,
  extreme and near-zero rates, the capacity boundary matrix, zero-time-delta, a
  non-advancing clock, multi-year advances, and concurrent acquire-while-reset.

## The clock is the floor

The bucket's own accounting is a few nanoseconds. End-to-end, `try_acquire` is
dominated by the monotonic clock read (`Instant::now()` via
[`clock-lib`](https://crates.io/crates/clock-lib)). On the same clock the bucket
ties the incumbent (`governor`); `governor` is faster out-of-the-box only because
its default clock (`quanta`, TSC-calibrated) is faster than `Instant`. Closing
that gap is a `clock-lib` concern — a faster monotonic source — not a bucket one.
The bucket is already at its floor.

The clock is **injected** (`Bucket<C: Clock>`), so a consumer or test supplies
its own — chiefly `clock-lib`'s `ManualClock`, which makes time-driven behaviour
deterministic with no `sleep`.

## Decisions, recorded

- **Token bucket is the sole algorithm.** Leaky-bucket and sliding-window
  limiting do not share the `TokenBucket` trait cleanly and belong in the
  downstream `rate-net`. This crate owns token-bucket accounting and nothing
  else.
- **Plain `u32`, not `NonZeroU32`, on the Tier-1 path.** Friendlier than the
  incumbent's API; a zero argument yields a well-defined deny-all bucket rather
  than forcing the caller to wrap every literal. The validated, error-returning
  path is `BucketConfig::new` / the builder.
- **No keyed store here.** Per-key state (the sharded map, eviction, quotas) is
  the consumer's concern. This crate is the single-bucket primitive.
- **Zero `unsafe` on the public path.** The packing trick needs none.
- **No SIMD.** A single-bucket acquire is one CAS on one word — no
  lane-parallelism to exploit, and no consumer needs batched multi-bucket
  acquire.

---

<sub>Copyright &copy; 2026 <strong>James Gober</strong>. All rights reserved.</sub>
