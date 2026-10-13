# better-bucket — Design

How the bucket works inside, and why it is built the way it is. For the public
API see [`API.md`](API.md); for numbers see [`BENCHMARKS.md`](BENCHMARKS.md).

## The one invariant

Everything here serves a single safety contract: **the bucket never
over-grants.** Under any interleaving of concurrent callers, the tokens granted
over any window of time never exceed the capacity plus what the configured rate
accrues over that window, and the live token count always stays within
`[0, capacity]`. Every design choice below is in service of making that true and
cheap.

`1.0.0` stated that contract and broke it: under ordinary multi-threaded use
with the default clock it granted 36x–527x its quota (see
[the 1.1.0 notes](release/v1.1.0.md)). The representation below is the fix, and
the reason the fix needs no tuning.

## State: one instant in one atomic word

A token bucket of capacity `C` refilling at rate `r` can be described by a
single instant `z`: the moment the bucket would have been empty. At time `now`
it holds

```
tokens(now) = min(C, (now − z) · r)
```

and granting `n` tokens moves `z` forward:

```
z ← max(z, now − C/r) + n/r        allowed only if the new z ≤ now
```

The `max` clamps an idle bucket to "full"; the condition says the tokens must
already have accrued. This is the token-bucket reading of GCRA, and the whole
mutable state is that one instant, held in a single `AtomicU64`:

```
 63                                                                 0
┌───────────────────────────────────────────────────────────────────┐
│   z: the empty-instant, in units since the bucket's epoch          │
└───────────────────────────────────────────────────────────────────┘
```

Keeping everything in one word is what makes the acquire path a single
compare-and-swap. A design with two atomics (tokens and time separately) cannot
update them together atomically, which reopens exactly the lost-update and
over-grant races this crate exists to prevent.

### Why not `(tokens, last_refill)`

`1.0` packed a token count and a 32-bit "last refill" millisecond into the word
and computed the refill as `now_ms.wrapping_sub(last_ms)`. Each thread read the
clock before its CAS loop. A thread whose reading was older than the timestamp
another thread had just committed got a "negative" elapsed time, which wrapped
to ~49.7 days, refilled the bucket to full, and wrote the older timestamp back,
so the next caller re-credited the same interval. The 32-bit time field also
wrapped every 49.7 days, which forced the `wrapping_sub` in the first place, and
every grant discarded the sub-millitoken refill remainder.

Patching that design means deciding how far "behind" a reading may be before it
is treated as skew rather than a long idle gap — a window that either stalls
long-idle buckets or leaves a smaller over-grant. The single-instant state needs
no such window. A grant only ever moves `z` forward. A stale reading, or a clock
that steps backwards, makes `now − z` *smaller*: the caller sees fewer tokens and
is denied, never granted more. There is no subtraction that can wrap, because
nothing wraps.

## Units: exact tokens, a converted clock

`z` and `now` are measured in **units** of `2^-unit_shift` tokens, chosen per
bucket at construction.

- **Token amounts are exact.** A grant of `n` tokens costs `n << unit_shift`
  units, the capacity is `C << unit_shift`, a refund adds back exactly what a
  grant took. With the clock frozen, every token is conserved — granted, still
  available, or refunded — with no rounding at all. The 16-thread conservation
  test checks exactly this.
- **The clock is converted once per call.** A reading in nanoseconds becomes
  units as the high word of one 64×64-bit multiply: `units = (ns · rate_q) >> 64`,
  where `rate_q / 2^64` is the refill rate in units per nanosecond, rounded up.
  The conversion depends only on the absolute reading, not on how many grants
  happened, so its error never accumulates: over the whole clock range the
  converted reading is less than one unit ahead of exact.

`unit_shift` is the finest the word allows under two limits: the capacity must
fit in 61 bits of units, and the clock must advance less than one unit per
nanosecond, so that the word lasts at least ~438 years before the reading
saturates (and `rate_q` fits in 64 bits). For every rate below 10^9
tokens/second a unit is at most one token, so grants are exact. Faster buckets
count in multi-token units and round each grant up to whole units — always
charging the caller, never the bucket. For typical buckets the clock limit
binds and one unit is between one and two nanoseconds of refill; for a bucket
whose capacity is large relative to its rate the capacity limit binds and one
unit is at most 2^-29 of a token.

Every rounding in the system points the same way:

| Quantity | Rounding | Effect |
|---|---|---|
| Grant cost (only for ≥ 10^9 tokens/s) | up | a grant may cost up to one unit more |
| Rate `rate_q` | up | the converted reading is less than one unit ahead of exact |
| Clock reading → units | down | a token accrues at the first whole unit after it is due |
| Full-bucket clamp | down, to the unit | a bucket that sat full is credited up to one unit early |
| `retry_after` / `time_until` | up to the millisecond | waiting the hint is always enough |

The rate is rounded *up* deliberately. An exact decimal boundary — 500 ms into a
10/s bucket, where five tokens are exactly due — then lands on or just past the
whole unit, so the grant happens at 500 ms and the hint says 500 ms. Rounding the
rate down instead would push every such boundary one nanosecond late and turn
every exact `retry_after` into the next millisecond up. Together with the
full-bucket clamp, the cost is that a decision can be up to two units early: a
token may be granted a few nanoseconds before it is exactly due (or, for a large
slow bucket, up to 2^-28 of a token early). That is the only slack in the
no-over-grant contract, and it is below the resolution of any real clock.

Instants are offset by 2^62 units so that the initial state (`now − cost(initial)`)
and the full-bucket clamp (`now − burst`) can never underflow.

## Acquire: a CAS loop

`try_acquire(n)` reads the clock once, converts it to units, then:

1. Load the word: `z`.
2. Compute `next = max(z, now − burst) + cost(n)`.
3. If `next > now`, the tokens have not accrued: return a denial — **without
   writing**, so a denied request never contends with a granting one. The
   denial's `retry_after` is the time until the clock reaches `next`.
4. Otherwise `compare_exchange_weak(z, next)`; on success, allow. On failure
   (another thread won the race, or a spurious weak failure), retry from step 2
   with the value that won, after a `spin_loop` hint.

There is no lock, no allocation, no division, and no syscall on the success path
beyond the monotonic clock read. A retry does not re-read the clock: a stale
reading is safe, and a fresh one would cost a clock read per retry under exactly
the contention where it hurts most.

### Memory ordering

The loads and the CAS use `Relaxed`. The decision depends only on the one word,
and the no-over-grant contract depends only on the read-modify-write
*atomicity* of updates to it, which a `Relaxed` CAS provides: a single atomic
location has a total modification order, and every successful CAS moves `z` up
by at least the cost it grants. Ordering the clock read against other threads'
commits — which the `1.0` design needed and lacked — buys nothing here, because
any reading, however stale, can only deny more. Stronger ordering would add
barriers on weakly-ordered targets for no change in what can be granted. `loom`
checks this with the clock itself modelled as a shared atomic.

## Time: nanoseconds, no wrap

`Bucket` measures time as the nanoseconds elapsed since it was built
(`clock.now()` against a stored `Monotonic` anchor); `BucketCore` takes the
caller's `Duration` since any epoch. Either way the reading saturates at
~584 years and the unit clock at ≥ ~438 years; past that the bucket simply stops
refilling. Nothing wraps, so an idle gap of any length — a minute, 49.7 days,
a decade — refills exactly as configured. `1.0`'s once-per-49.7-days
under-refill and its "fills to capacity only by luck of the modulus" long-idle
test are gone.

A reading earlier than the bucket's anchor counts as the anchor: `Bucket` uses
`saturating_duration_since`, so a reading taken just before construction can
never panic or refill.

## Reservations, refunds, partial grants

The single-instant state makes three operations that a `(tokens, time)` word
could not support cleanly fall out for free:

- **`reserve(n, max_wait)`** takes the tokens immediately even if they have not
  accrued, moving `z` past `now`, and returns how long until they are covered.
  Later callers see `z > now` and queue behind it. This is what prevents a large
  request from being starved by a stream of small ones (a plain token bucket has
  no queue, so it is starvable by design). `max_wait` bounds how far into the
  future a caller can push the bucket.
- **`refund(n)`** moves `z` back by `cost(n)`. The `max(z, now − burst)` clamp on
  every read caps the visible balance at the capacity, so an over-refund cannot
  overfill the bucket. Refunding tokens that were never acquired is the caller's
  decision and lies outside the over-grant contract, as does `reset`.
- **`acquire_up_to(n)`** grants `min(n, available)` in one CAS — the
  byte-throttling shape.

## Overflow safety

Every computation is checked, saturating, or widened to `u128`. A request above
the capacity is rejected before any arithmetic. Costs stay below 2^61 because
the capacity does; the clock product `ns · rate_q` is below 2^127; waits that
would end beyond the clock's range are reported as `Duration::MAX` ("never")
rather than computed. Construction rejects nonsensical configurations (zero
capacity, amount, or period) up front with a `BucketError`; the Tier-1
constructors stay infallible by interpreting a degenerate argument as a bucket
that grants nothing. There is no configuration `BucketConfig::new` accepts that
the bucket cannot represent exactly: the 4.29 M-token capacity clamp and the
"rates slower than ~1 per 48.5 days never refill" floor of `1.0` are gone.

## How the contract is defended

- **`loom`** model-checks the real `Bucket` and `BucketCore`, with the clock
  modelled as a loom atomic so it explores every ordering of clock reads and
  advances against the CAS: a reader overtaken by a thread that advanced the
  clock and committed (the `1.0` bug, which this model finds immediately on the
  old code), two advances interleaved with three acquires, caller-supplied time
  read from a shared atomic, `reset` and `refund` racing acquires, and a
  reservation racing a grant.
- **A real-clock contention test** runs 2–16 threads against a `SystemClock`
  bucket and asserts the total granted against the ceiling the configuration
  allows over the measured wall time. `1.0` fails it by thousands of tokens.
- **Deterministic clock tests** pause a thread between its clock read and its
  CAS while another drains the bucket, and step a custom clock backwards; both
  must grant nothing extra.
- **`proptest`** drives `BucketCore` with arbitrary rates and arbitrary time
  steps, including backwards ones, and checks: no window of decisions exceeds
  capacity plus accrual; backward readings never add tokens; a frozen clock
  conserves every token through acquires, partial grants and refunds;
  `available` is the exact grant boundary; the retry hint equals `time_until`
  and is honest; reservations never commit more than is covered by their wait.
- **An allocation audit** runs every per-call operation under a per-thread
  counting allocator and asserts zero allocations.
- **An adversarial/edge suite** covers `u32::MAX` requests and capacities,
  extreme and near-zero rates, idle gaps across the old 2^31/2^32 ms
  boundaries, multi-century readings, the capacity boundary matrix, and
  concurrent acquire-while-reset with a bounded total.
- **Miri** runs the unit, property, and edge suites.

## The clock is the floor

The bucket's own accounting is a few nanoseconds. End-to-end, `try_acquire` is
dominated by the monotonic clock read (`Instant::now()` via
[`clock-lib`](https://crates.io/crates/clock-lib)). Against `governor` on the
same clock, `governor` is a little faster per call — it is GCRA, which tracks
the same single instant but has no token semantics to compute. Out of the box
`governor` is faster still because its default `quanta` clock beats `Instant`.
The remaining end-to-end gap is the clock, a `clock-lib` concern (a faster
monotonic source), not a bucket one. See [`BENCHMARKS.md`](BENCHMARKS.md) for
the numbers.

The clock is **injected** (`Bucket<C: Clock>`), so a consumer or test supplies
its own — chiefly `clock-lib`'s `ManualClock`, which makes time-driven behaviour
deterministic with no `sleep`. The `*_at` methods take a reading instead, so a
keyed limiter can read the clock once per request and decide for many buckets.
`BucketCore` drops the clock altogether.

## `no_std`

The engine needs only `core` and a 64-bit atomic. Without `std`, the crate
exposes `BucketCore` (caller-driven time), `BucketConfig`, `BucketBuilder`'s
`build_config`, `Decision`, and `BucketError` (which implements
`core::error::Error`). Targets with no native 64-bit atomics — Cortex-M among
them — get `BucketCore` through the optional `portable-atomic` feature; without
it they still get the configuration and decision types.

## Decisions, recorded

- **Token bucket is the sole algorithm.** Leaky-bucket and sliding-window
  limiting do not share the `TokenBucket` trait cleanly and belong in the
  downstream `rate-net`. This crate owns token-bucket accounting and nothing
  else.
- **A single instant, not `(tokens, time)`.** Stale and backward readings become
  harmless by construction, time stops wrapping, and capacity and slow rates
  stop clamping (see above).
- **Exact token units, a rounded-up clock.** Token conservation is exact; the
  only rounding is a sub-nanosecond-per-decade fast clock, chosen so exact
  boundaries are hit rather than missed.
- **`retry_after` stays at millisecond resolution**, rounded up, as `1.0`
  promised. It is an honest upper bound, and what `Retry-After` headers and most
  timers consume.
- **Plain `u32`, not `NonZeroU32`, on the Tier-1 path.** A zero argument yields a
  well-defined deny-all bucket rather than forcing the caller to wrap every
  literal. The validated, error-returning path is `BucketConfig::new` / the
  builder.
- **No keyed store here.** Per-key state (the sharded map, eviction, quotas) is
  the consumer's concern. This crate is the single-bucket primitive.
- **No blocking or async wait.** `reserve` returns the delay; the caller sleeps
  with whatever timer fits its runtime. Async waiting lives in `rate-net`.
- **Zero `unsafe`.** `#![forbid(unsafe_code)]`.
- **No SIMD.** A single-bucket acquire is one CAS on one word — no
  lane-parallelism to exploit.

---

<sub>Copyright &copy; 2026 <strong>James Gober</strong>. All rights reserved.</sub>
