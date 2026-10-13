//! The lock-free accounting engine and its clock-free public face, `BucketCore`.
//!
//! Everything that decides a grant lives here. [`Bucket`](crate::Bucket) is
//! this engine plus a clock; [`BucketCore`] is this engine with the caller
//! supplying the time, which is what makes the crate usable without `std`.
//!
//! # Representation
//!
//! A token bucket of capacity `C` refilling at rate `r` is equivalent to
//! tracking a single instant, `z`: the moment the bucket would have been empty.
//! Measured in *units* of accrued refill, at clock reading `now` it holds
//!
//! ```text
//! tokens(now) = min(C, now − z)        (in units)
//! ```
//!
//! and granting `n` tokens is `z ← max(z, now − C) + n`, allowed only if the
//! new `z` is not after `now`. This is the token-bucket reading of GCRA. The
//! whole mutable state is that one instant, held in a single [`AtomicU64`], so an
//! acquire is one `compare_exchange_weak`.
//!
//! The representation is what makes the bucket safe under contention. A grant
//! only ever moves `z` forward, and a caller whose `now` is stale (read before a
//! racing thread committed a later one), or a clock that steps backwards, can
//! only see *fewer* tokens: `now − z` shrinks, it never wraps. There is no
//! elapsed-time subtraction that can underflow into a phantom refill, and no
//! skew window to tune.
//!
//! # Units
//!
//! One unit is `2^-unit_shift` tokens, so token amounts — grants, refunds, the
//! capacity — are exact shifts whenever a unit is at most one token: there is
//! no rounding in the token accounting, and a bucket with no refill conserves
//! every token. `unit_shift` is chosen per bucket as fine as the word allows:
//!
//! - the capacity must fit in 61 bits of units;
//! - below 10^9 tokens/second the clock must advance less than one unit per
//!   nanosecond, so it lasts at least ~438 years before it saturates;
//! - at 10^9 tokens/second and above, a unit is one whole token as long as the
//!   clock stays at or below 4.3 units per nanosecond (at least ~100 years of
//!   range). That covers every `u32` amount per second. Only faster buckets
//!   (above ~4.3 × 10^9 tokens/second, which needs a sub-second period) count
//!   in multi-token units and round each grant up to a whole unit.
//!
//! The clock reading, in nanoseconds since the bucket's epoch, is converted to
//! units by multiplying by the rate, held as an integer part (`rate_int`, zero
//! below 10^9 tokens/second, at most 4) and a `Q64` fraction (`rate_frac`),
//! rounded **up**: the high word of one 64×64-bit multiply, plus one more
//! multiply only for the fastest buckets. The conversion is a function of the
//! absolute reading, not of the history, so its error never accumulates: over
//! the whole clock range the bucket's clock is less than one unit ahead of
//! exact, and an instant at which a token is exactly due (500 ms into a 10/s
//! bucket) is never missed. Instants are offset by [`ORIGIN`] so the initial
//! state never underflows. Nothing wraps; once the clock saturates it stops,
//! and a charge that would land past its end is refused.

use core::time::Duration;

#[cfg(all(not(loom), not(feature = "portable-atomic")))]
use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(loom)]
use loom::sync::atomic::{AtomicU64, Ordering};
#[cfg(all(not(loom), feature = "portable-atomic"))]
use portable_atomic::{AtomicU64, Ordering};

use crate::config::BucketConfig;
use crate::decision::Decision;
#[cfg(feature = "metrics")]
use crate::metrics::BucketMetrics;

/// Offset added to every instant, in units. Larger than any burst, so
/// `now − burst` and the initial `now − cost(initial)` never underflow.
const ORIGIN: u64 = 1 << 62;

/// Bits the capacity may occupy, in units: `capacity << unit_shift <= 2^61`.
const BURST_BITS: i32 = 61;

const NANOS_PER_SEC: u64 = 1_000_000_000;
const NANOS_PER_MILLI: u128 = 1_000_000;

/// Saturating nanoseconds in a `Duration`. Saturates at ~584 years.
#[inline]
fn duration_nanos(d: Duration) -> u64 {
    d.as_secs()
        .saturating_mul(NANOS_PER_SEC)
        .saturating_add(u64::from(d.subsec_nanos()))
}

/// Significant bits in `x` (`0` for zero).
fn bits(x: u128) -> i32 {
    // At most 128, which always fits an i32.
    i32::try_from(u128::BITS - x.leading_zeros()).unwrap_or(128)
}

/// The per-bucket unit and clock rate, derived once at construction.
#[derive(Clone, Copy)]
struct Scale {
    /// Whole units per nanosecond: zero below 10^9 tokens/second, at most 4.
    rate_int: u8,
    /// The fractional units per nanosecond in `Q64`. Together with
    /// `rate_int` the rate is rounded up and is never zero.
    rate_frac: u64,
    /// One unit is `2^-unit_shift` tokens. Negative only above ~4.3 × 10^9
    /// tokens/second.
    unit_shift: i8,
}

/// The fastest the clock may run when a unit is one token or more: 4.3 units
/// per nanosecond (`43 / 10`), enough for every `u32` amount per second while
/// keeping at least ~100 years of range.
const FAST_CLOCK_NUM: u128 = 43;
const FAST_CLOCK_DEN: u128 = 10;

impl Scale {
    /// The rate for a bucket that refills `amount` tokens every `period` and
    /// holds `capacity`. A degenerate bucket (any of them zero) gets a dummy
    /// rate: with capacity `0` every non-zero request is refused before the rate
    /// is consulted.
    fn derive(capacity: u32, amount: u32, period: Duration) -> Self {
        if capacity == 0 || amount == 0 || period.is_zero() {
            // A rate of 2^-64 units/ns keeps the clock at zero for 584 years:
            // frozen.
            return Self {
                rate_int: 0,
                rate_frac: 1,
                unit_shift: 0,
            };
        }
        let amount = u128::from(amount);
        let period_ns = period.as_nanos(); // in [1, 2^94)

        // The finest unit the capacity allows: capacity << shift < 2^61.
        let burst_limit = BURST_BITS - bits(u128::from(capacity));
        // The finest unit that keeps the clock below one unit per ns, i.e. the
        // largest m with amount · 2^m < period_ns.
        let below = |m: i32| {
            if m >= 0 {
                amount << m.unsigned_abs() < period_ns
            } else {
                amount < period_ns << m.unsigned_abs()
            }
        };
        let mut clock_limit = bits(period_ns) - bits(amount);
        while !below(clock_limit) {
            clock_limit -= 1;
        }
        if clock_limit < 0 {
            // 10^9 tokens/second or faster. Rather than coarsen the unit until
            // the clock is below one unit per ns, keep whole tokens (or the
            // finest multi-token unit) while the clock stays at or below
            // 4.3 units/ns: the largest m <= 0 with
            // 10 · amount · 2^m <= 43 · period_ns.
            let fits = |m: i32| {
                FAST_CLOCK_DEN * amount <= (FAST_CLOCK_NUM * period_ns) << m.unsigned_abs()
            };
            clock_limit = 0;
            while !fits(clock_limit) {
                clock_limit -= 1;
            }
        }
        let unit_shift = burst_limit.min(clock_limit);

        // rate = ceil(amount · 2^(unit_shift + 64) / period_ns), in Q64. A
        // period wider than 64 bits is first rounded *down* to 64 significant
        // bits, which can only raise the estimate, keeping the direction of
        // every rounding the same: the bucket's clock may run a hair fast,
        // never slow. The numerator stays below 2^128: below 10^9 tokens/s the
        // rate is under one unit per ns (amount · 2^unit_shift < period_ns);
        // above it the period has at most 32 bits, nothing is dropped, and
        // unit_shift <= 0. The shift is positive because unit_shift >= -32 and
        // the period has at most 95 bits.
        let drop = (bits(period_ns) - 64).max(0);
        let period_top = period_ns >> drop.unsigned_abs();
        let shift = unit_shift + 64 - drop;
        let rate = (amount << shift.unsigned_abs()).div_ceil(period_top);
        Self {
            rate_int: u8::try_from(rate >> 64).unwrap_or(u8::MAX),
            rate_frac: u64::try_from(rate & u128::from(u64::MAX)).unwrap_or(u64::MAX),
            unit_shift: i8::try_from(unit_shift).unwrap_or(i8::MIN),
        }
    }
}

/// Allowed / denied / granted-token counters, behind the `metrics` feature.
#[cfg(feature = "metrics")]
struct Counters {
    allowed: AtomicU64,
    denied: AtomicU64,
    tokens_granted: AtomicU64,
}

/// The shared engine: the packed state plus the immutable rate and the
/// configuration it was built from. 48 bytes without `metrics`.
pub(crate) struct Core {
    /// The empty-instant `z`, in units offset by [`ORIGIN`]. The only mutable
    /// accounting state and the single point of synchronisation.
    state: AtomicU64,
    /// See [`Scale::rate_frac`].
    rate_frac: u64,
    /// `refill_period`, split so the struct has no padding hole.
    period_secs: u64,
    capacity: u32,
    refill_amount: u32,
    initial: u32,
    period_nanos: u32,
    /// See [`Scale::rate_int`].
    rate_int: u8,
    /// See [`Scale::unit_shift`].
    unit_shift: i8,
    #[cfg(feature = "metrics")]
    counters: Counters,
}

impl Core {
    /// Builds the engine with `config.initial()` tokens as of `now`.
    pub(crate) fn new(config: BucketConfig, now: Duration) -> Self {
        let rate = Scale::derive(
            config.capacity(),
            config.refill_amount(),
            config.refill_period(),
        );
        let period = config.refill_period();
        let core = Self {
            state: AtomicU64::new(0),
            rate_frac: rate.rate_frac,
            period_secs: period.as_secs(),
            capacity: config.capacity(),
            refill_amount: config.refill_amount(),
            initial: config.initial(),
            period_nanos: period.subsec_nanos(),
            rate_int: rate.rate_int,
            unit_shift: rate.unit_shift,
            #[cfg(feature = "metrics")]
            counters: Counters {
                allowed: AtomicU64::new(0),
                denied: AtomicU64::new(0),
                tokens_granted: AtomicU64::new(0),
            },
        };
        let initial = core.cost(config.initial().min(config.capacity()));
        core.state
            .store(core.units(now).saturating_sub(initial), Ordering::Relaxed);
        core
    }

    /// The configuration this engine was built from.
    #[inline]
    pub(crate) const fn config(&self) -> BucketConfig {
        BucketConfig::raw(
            self.capacity,
            self.refill_amount,
            Duration::new(self.period_secs, self.period_nanos),
            self.initial,
        )
    }

    #[inline]
    pub(crate) const fn capacity(&self) -> u32 {
        self.capacity
    }

    /// The clock reading `now` as an offset instant in units: the high word of
    /// one 64×64-bit multiply, plus one more multiply for buckets of 10^9
    /// tokens/second and above. Saturates (the clock stops) after at least
    /// ~100 years (~438 years below 10^9 tokens/second); never wraps.
    #[inline]
    fn units(&self, now: Duration) -> u64 {
        let ns = u128::from(duration_nanos(now));
        let mut accrued = (ns * u128::from(self.rate_frac)) >> 64;
        if self.rate_int != 0 {
            // At most 2^64 · 4, so the sum stays far inside a u128.
            accrued += ns * u128::from(self.rate_int);
        }
        ORIGIN.saturating_add(u64::try_from(accrued).unwrap_or(u64::MAX))
    }

    /// Units that `n` tokens cost. Exact when a unit is at most one token (every
    /// rate up to ~4.3 × 10^9 tokens/second); rounded up to whole units for
    /// faster buckets. For `n <= capacity` this is at most the burst, below
    /// 2^61.
    #[inline]
    fn cost(&self, n: u32) -> u64 {
        let n = u64::from(n);
        let shift = self.unit_shift.unsigned_abs();
        if self.unit_shift >= 0 {
            n << shift
        } else if n == 0 {
            0
        } else {
            ((n - 1) >> shift) + 1
        }
    }

    /// Units in a full bucket: the cost of a full-capacity grant.
    #[inline]
    fn burst(&self) -> u64 {
        self.cost(self.capacity)
    }

    /// Whole tokens bought by `credit` units (`credit <= burst`): the largest
    /// `n` with `cost(n) <= credit`, capped at capacity.
    #[inline]
    fn tokens_for(&self, credit: u64) -> u32 {
        let shift = self.unit_shift.unsigned_abs();
        let tokens = if self.unit_shift >= 0 {
            credit >> shift
        } else {
            // credit <= burst <= 2^32 >> shift, so this stays below 2^33.
            credit << shift
        };
        u32::try_from(tokens).unwrap_or(u32::MAX).min(self.capacity)
    }

    /// The wait from `now` until the bucket's clock reaches `target`, rounded
    /// up to the millisecond. `None` if `target` lies beyond the clock's range,
    /// i.e. the wait can never end.
    fn wait_until(&self, target: u64, now: Duration) -> Option<Duration> {
        // The first nanosecond reading `e` with units(e) >= target is
        // ceil(d · 2^64 / rate), where d < 2^64 is target's offset and the
        // Q64 rate is at least 1.
        let rate = (u128::from(self.rate_int) << 64) | u128::from(self.rate_frac);
        let d = u128::from(target.saturating_sub(ORIGIN));
        let due_ns = (d << 64).div_ceil(rate);
        // Readings saturate at u64::MAX ns, so anything later never arrives.
        if due_ns > u128::from(u64::MAX) {
            return None;
        }
        let wait_ns = due_ns.saturating_sub(u128::from(duration_nanos(now)));
        let millis = wait_ns.div_ceil(NANOS_PER_MILLI);
        Some(Duration::from_millis(
            u64::try_from(millis).unwrap_or(u64::MAX),
        ))
    }

    /// The single grant path behind every acquire. One relaxed load, then a
    /// `compare_exchange_weak` loop that only retries if another thread changed
    /// the word in between.
    #[inline]
    pub(crate) fn acquire(&self, n: u32, now: Duration) -> Decision {
        if n == 0 {
            // Zero tokens are always available, even from an empty bucket.
            self.record_allowed(0);
            return Decision::Allowed;
        }
        if n > self.capacity {
            // More than the bucket can ever hold: it can never be granted.
            self.record_denied();
            return Decision::Denied {
                retry_after: Duration::MAX,
            };
        }
        let now_u = self.units(now);
        let cost = self.cost(n);
        let floor = now_u.saturating_sub(self.burst());
        let mut current = self.state.load(Ordering::Relaxed);
        loop {
            // Clamp to "full" (z no earlier than now − burst), then charge. An
            // overflow means the charge lands past the end of the clock's
            // range: it can never be covered, so it fails closed.
            let next = match current.max(floor).checked_add(cost) {
                Some(next) if next <= now_u => next,
                overflowed_or_short => {
                    // Denied: report the wait for the shortfall to accrue. No
                    // write, so a denied request never contends with a granting
                    // one.
                    self.record_denied();
                    let retry_after = overflowed_or_short
                        .and_then(|next| self.wait_until(next, now))
                        .unwrap_or(Duration::MAX);
                    return Decision::Denied { retry_after };
                }
            };
            // Relaxed is sufficient: the decision depends only on this one word,
            // and the CAS gives the read-modify-write atomicity the
            // no-over-grant contract needs. `now` needs no ordering against
            // other threads, because a stale `now` can only deny more.
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.record_allowed(n);
                    return Decision::Allowed;
                }
                // Lost the race (or a spurious weak failure): retry against the
                // value that won, with a spin hint to ease cache-line pressure.
                Err(actual) => {
                    current = actual;
                    core::hint::spin_loop();
                }
            }
        }
    }

    /// Grants as many of `n` tokens as are available, returning the count.
    pub(crate) fn acquire_up_to(&self, n: u32, now: Duration) -> u32 {
        if n == 0 {
            self.record_allowed(0);
            return 0;
        }
        let want = n.min(self.capacity);
        let now_u = self.units(now);
        let burst = self.burst();
        let floor = now_u.saturating_sub(burst);
        let mut current = self.state.load(Ordering::Relaxed);
        loop {
            let base = current.max(floor);
            let credit = now_u.saturating_sub(base);
            let take = if credit >= burst {
                self.capacity
            } else {
                self.tokens_for(credit)
            }
            .min(want);
            if take == 0 {
                self.record_denied();
                return 0;
            }
            // cost(take) <= credit by construction of `tokens_for`.
            let next = base.saturating_add(self.cost(take));
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.record_allowed(take);
                    return take;
                }
                Err(actual) => {
                    current = actual;
                    core::hint::spin_loop();
                }
            }
        }
    }

    /// Whole tokens available as of `now`.
    pub(crate) fn available(&self, now: Duration) -> u32 {
        if self.capacity == 0 {
            return 0;
        }
        let now_u = self.units(now);
        let burst = self.burst();
        let base = self
            .state
            .load(Ordering::Relaxed)
            .max(now_u.saturating_sub(burst));
        let credit = now_u.saturating_sub(base);
        if credit >= burst {
            self.capacity
        } else {
            self.tokens_for(credit)
        }
    }

    /// The wait until `n` tokens would be grantable, without taking any.
    pub(crate) fn time_until(&self, n: u32, now: Duration) -> Option<Duration> {
        if n == 0 {
            return Some(Duration::ZERO);
        }
        if n > self.capacity {
            return None;
        }
        let now_u = self.units(now);
        let next = self
            .state
            .load(Ordering::Relaxed)
            .max(now_u.saturating_sub(self.burst()))
            .checked_add(self.cost(n))?;
        if next <= now_u {
            Some(Duration::ZERO)
        } else {
            self.wait_until(next, now)
        }
    }

    /// Takes `n` tokens now, possibly ahead of their accrual, if the wait until
    /// they are covered is at most `max_wait`. Returns that wait.
    pub(crate) fn reserve(&self, n: u32, max_wait: Duration, now: Duration) -> Option<Duration> {
        if n == 0 {
            self.record_allowed(0);
            return Some(Duration::ZERO);
        }
        if n > self.capacity {
            self.record_denied();
            return None;
        }
        let now_u = self.units(now);
        let cost = self.cost(n);
        let floor = now_u.saturating_sub(self.burst());
        let mut current = self.state.load(Ordering::Relaxed);
        loop {
            // As in `acquire`, a charge past the end of the clock's range can
            // never be covered: fail closed.
            let Some(next) = current.max(floor).checked_add(cost) else {
                self.record_denied();
                return None;
            };
            let wait = if next <= now_u {
                Some(Duration::ZERO)
            } else {
                self.wait_until(next, now)
            };
            let Some(wait) = wait.filter(|wait| *wait <= max_wait) else {
                self.record_denied();
                return None;
            };
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.record_allowed(n);
                    return Some(wait);
                }
                Err(actual) => {
                    current = actual;
                    core::hint::spin_loop();
                }
            }
        }
    }

    /// Returns up to `n` tokens to the bucket (never beyond capacity).
    pub(crate) fn refund(&self, n: u32) {
        let n = n.min(self.capacity);
        if n == 0 {
            return;
        }
        let cost = self.cost(n);
        let mut current = self.state.load(Ordering::Relaxed);
        loop {
            // Moving `z` earlier adds credit; the `max(z, now − burst)` clamp on
            // every read caps the visible balance at capacity.
            let next = current.saturating_sub(cost);
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(actual) => {
                    current = actual;
                    core::hint::spin_loop();
                }
            }
        }
    }

    /// Refills to capacity as of `now`, cancelling any outstanding reservation.
    pub(crate) fn reset(&self, now: Duration) {
        self.state.store(
            self.units(now).saturating_sub(self.burst()),
            Ordering::Relaxed,
        );
    }

    #[inline]
    fn record_allowed(&self, tokens: u32) {
        #[cfg(feature = "metrics")]
        {
            let _ = self.counters.allowed.fetch_add(1, Ordering::Relaxed);
            let _ = self
                .counters
                .tokens_granted
                .fetch_add(u64::from(tokens), Ordering::Relaxed);
        }
        #[cfg(not(feature = "metrics"))]
        let _ = tokens;
    }

    #[inline]
    fn record_denied(&self) {
        #[cfg(feature = "metrics")]
        {
            let _ = self.counters.denied.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg(feature = "metrics")]
    pub(crate) fn metrics(&self) -> BucketMetrics {
        BucketMetrics::new(
            self.counters.allowed.load(Ordering::Relaxed),
            self.counters.denied.load(Ordering::Relaxed),
            self.counters.tokens_granted.load(Ordering::Relaxed),
        )
    }
}

/// A token bucket that takes the current time from the caller.
///
/// `BucketCore` is the same lock-free engine as [`Bucket`](crate::Bucket),
/// without a clock. Every time-dependent method takes `now`: the time elapsed
/// since an epoch of your choosing (boot, a timer peripheral's zero, a test's
/// start), the same epoch for every call on one bucket. That makes it the
/// `no_std` path — it needs only `core` and a 64-bit atomic — and the
/// deterministic one: there is nothing to mock.
///
/// The bucket is created holding [`initial`](BucketConfig::initial) tokens as
/// of the `now` passed to [`new`](Self::new), so the epoch does not have to be
/// the bucket's creation time.
///
/// `now` should not run backwards. If it does (a stale reading, a reset timer),
/// the bucket simply sees fewer tokens until time catches up again — it never
/// grants more because of it.
///
/// # Examples
///
/// ```
/// use better_bucket::{BucketConfig, BucketCore};
/// use core::time::Duration;
///
/// // 10 tokens/second, burst 10, starting full at t = 0.
/// let config = BucketConfig::new(10, 10, Duration::from_secs(1), 10)?;
/// let bucket = BucketCore::new(config, Duration::ZERO);
///
/// assert!(bucket.try_acquire_at(10, Duration::ZERO)); // drain
/// assert!(!bucket.try_acquire_at(1, Duration::ZERO)); // empty
///
/// // Half a second later, five tokens have accrued.
/// assert_eq!(bucket.available_at(Duration::from_millis(500)), 5);
/// # Ok::<(), better_bucket::BucketError>(())
/// ```
#[repr(align(64))]
pub struct BucketCore {
    core: Core,
}

impl BucketCore {
    /// Creates a bucket from `config`, holding its initial tokens as of `now`.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// // Created 5 s into the caller's timeline, starting empty.
    /// let config = BucketConfig::new(100, 100, Duration::from_secs(1), 0)?;
    /// let bucket = BucketCore::new(config, Duration::from_secs(5));
    /// assert_eq!(bucket.available_at(Duration::from_secs(5)), 0);
    /// assert_eq!(bucket.available_at(Duration::from_millis(5_250)), 25);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[must_use]
    pub fn new(config: BucketConfig, now: Duration) -> Self {
        Self {
            core: Core::new(config, now),
        }
    }

    /// Attempts to take `n` tokens at time `now`, returning the full
    /// [`Decision`].
    ///
    /// Same contract as [`Bucket::acquire`](crate::Bucket::acquire): `0`
    /// always succeeds, more than the capacity never does
    /// (`retry_after == Duration::MAX`), and a denial leaves the bucket
    /// untouched.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore, Decision};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(2, 2, Duration::from_secs(1), 2)?;
    /// let bucket = BucketCore::new(config, Duration::ZERO);
    /// assert_eq!(bucket.acquire_at(2, Duration::ZERO), Decision::Allowed);
    /// assert_eq!(
    ///     bucket.acquire_at(1, Duration::ZERO),
    ///     Decision::Denied { retry_after: Duration::from_millis(500) }
    /// );
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[inline]
    pub fn acquire_at(&self, n: u32, now: Duration) -> Decision {
        self.core.acquire(n, now)
    }

    /// Attempts to take `n` tokens at time `now`, returning whether it
    /// succeeded.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(1, 1, Duration::from_secs(1), 1)?;
    /// let bucket = BucketCore::new(config, Duration::ZERO);
    /// assert!(bucket.try_acquire_at(1, Duration::ZERO));
    /// assert!(!bucket.try_acquire_at(1, Duration::from_millis(999)));
    /// assert!(bucket.try_acquire_at(1, Duration::from_secs(1)));
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[inline]
    #[must_use]
    pub fn try_acquire_at(&self, n: u32, now: Duration) -> bool {
        self.core.acquire(n, now).is_allowed()
    }

    /// Takes as many of `n` tokens as are available at `now` (possibly zero)
    /// and returns how many it took.
    ///
    /// Useful for byte-oriented throttling, where a partial grant is still
    /// progress. Never takes more than the capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(100, 100, Duration::from_secs(1), 30)?;
    /// let bucket = BucketCore::new(config, Duration::ZERO);
    /// assert_eq!(bucket.acquire_up_to_at(50, Duration::ZERO), 30);
    /// assert_eq!(bucket.acquire_up_to_at(50, Duration::ZERO), 0);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[must_use = "the returned count is how many tokens were actually taken"]
    pub fn acquire_up_to_at(&self, n: u32, now: Duration) -> u32 {
        self.core.acquire_up_to(n, now)
    }

    /// Returns how many whole tokens are available at `now`.
    ///
    /// A snapshot: under concurrent acquires it can be stale the instant it
    /// returns.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(10, 10, Duration::from_secs(1), 10)?;
    /// let bucket = BucketCore::new(config, Duration::ZERO);
    /// assert!(bucket.try_acquire_at(4, Duration::ZERO));
    /// assert_eq!(bucket.available_at(Duration::ZERO), 6);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[must_use]
    pub fn available_at(&self, now: Duration) -> u32 {
        self.core.available(now)
    }

    /// Returns how long from `now` until `n` tokens could be acquired, without
    /// taking any.
    ///
    /// `Some(Duration::ZERO)` means they are available now; `Some(wait)` is the
    /// same honest, millisecond-rounded-up hint a denial carries; `None` means
    /// the request can never succeed (`n` exceeds the capacity).
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(10, 10, Duration::from_secs(1), 0)?;
    /// let bucket = BucketCore::new(config, Duration::ZERO);
    /// assert_eq!(bucket.time_until_at(5, Duration::ZERO), Some(Duration::from_millis(500)));
    /// assert_eq!(bucket.time_until_at(11, Duration::ZERO), None);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[must_use]
    pub fn time_until_at(&self, n: u32, now: Duration) -> Option<Duration> {
        self.core.time_until(n, now)
    }

    /// Reserves `n` tokens at `now`, even if they have not accrued yet, as long
    /// as the wait until they are covered is at most `max_wait`.
    ///
    /// On success the tokens are taken immediately and the returned duration is
    /// how long the caller should wait before acting on them (zero if they were
    /// already available). Later requests queue behind the reservation, which
    /// is what makes this fair for large requests that a stream of small ones
    /// would otherwise starve. Returns `None`, reserving nothing, if `n` exceeds
    /// the capacity or the wait would exceed `max_wait`.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(10, 10, Duration::from_secs(1), 0)?;
    /// let bucket = BucketCore::new(config, Duration::ZERO);
    /// let max = Duration::from_secs(2);
    ///
    /// // Empty bucket: 10 tokens are covered after one second.
    /// assert_eq!(bucket.reserve_at(10, max, Duration::ZERO), Some(Duration::from_secs(1)));
    /// // The next 10 queue behind them.
    /// assert_eq!(bucket.reserve_at(10, max, Duration::ZERO), Some(Duration::from_secs(2)));
    /// // A third reservation would wait 3 s, over the limit: nothing reserved.
    /// assert_eq!(bucket.reserve_at(10, max, Duration::ZERO), None);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[must_use = "the returned wait must be honoured before using the tokens"]
    pub fn reserve_at(&self, n: u32, max_wait: Duration, now: Duration) -> Option<Duration> {
        self.core.reserve(n, max_wait, now)
    }

    /// Returns `n` previously acquired tokens to the bucket.
    ///
    /// Use it when work that acquired tokens was cancelled or failed before it
    /// consumed the budget. The balance never exceeds the capacity, and a
    /// refund also pays down an outstanding reservation. Refunding tokens that
    /// were never acquired lets the bucket grant more than its rate — that is
    /// the caller's decision, not a bucket fault.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(10, 10, Duration::from_secs(1), 10)?;
    /// let bucket = BucketCore::new(config, Duration::ZERO);
    /// assert!(bucket.try_acquire_at(8, Duration::ZERO));
    /// bucket.refund(3); // the work for 3 of them was cancelled
    /// assert_eq!(bucket.available_at(Duration::ZERO), 5);
    /// bucket.refund(100); // capped at capacity
    /// assert_eq!(bucket.available_at(Duration::ZERO), 10);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    pub fn refund(&self, n: u32) {
        self.core.refund(n);
    }

    /// Refills the bucket to capacity as of `now`, discarding any debt or
    /// outstanding reservation.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(4, 4, Duration::from_secs(1), 4)?;
    /// let bucket = BucketCore::new(config, Duration::ZERO);
    /// assert!(bucket.try_acquire_at(4, Duration::ZERO));
    /// bucket.reset_at(Duration::ZERO);
    /// assert_eq!(bucket.available_at(Duration::ZERO), 4);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    pub fn reset_at(&self, now: Duration) {
        self.core.reset(now);
    }

    /// Returns the bucket's capacity (its burst ceiling), in whole tokens.
    ///
    /// Always equal to `self.config().capacity()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(64, 1, Duration::from_secs(1), 0)?;
    /// assert_eq!(BucketCore::new(config, Duration::ZERO).capacity(), 64);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[must_use]
    pub const fn capacity(&self) -> u32 {
        self.core.capacity()
    }

    /// Returns the configuration this bucket was built from.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(5, 1, Duration::from_millis(200), 5)?;
    /// assert_eq!(BucketCore::new(config, Duration::ZERO).config(), config);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[must_use]
    pub const fn config(&self) -> BucketConfig {
        self.core.config()
    }

    /// Returns a snapshot of the bucket's decision counters.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketConfig, BucketCore};
    /// use core::time::Duration;
    ///
    /// let config = BucketConfig::new(1, 1, Duration::from_secs(1), 1)?;
    /// let bucket = BucketCore::new(config, Duration::ZERO);
    /// let _ = bucket.try_acquire_at(1, Duration::ZERO);
    /// let _ = bucket.try_acquire_at(1, Duration::ZERO);
    /// let m = bucket.metrics();
    /// assert_eq!((m.allowed, m.denied, m.tokens_granted), (1, 1, 1));
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[cfg(feature = "metrics")]
    #[cfg_attr(docsrs, doc(cfg(feature = "metrics")))]
    #[must_use]
    pub fn metrics(&self) -> BucketMetrics {
        self.core.metrics()
    }
}

impl core::fmt::Debug for BucketCore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BucketCore")
            .field("capacity", &self.capacity())
            .field("config", &self.config())
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{BURST_BITS, BucketCore, Core, ORIGIN, Scale};
    use crate::config::BucketConfig;
    use crate::decision::Decision;
    use core::time::Duration;
    use std::format;

    const T0: Duration = Duration::ZERO;

    fn core(capacity: u32, amount: u32, period: Duration, initial: u32) -> BucketCore {
        BucketCore::new(
            BucketConfig::new(capacity, amount, period, initial).unwrap(),
            T0,
        )
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn test_core_starts_with_initial_tokens() {
        assert_eq!(core(10, 10, ms(1_000), 10).available_at(T0), 10);
        assert_eq!(core(10, 10, ms(1_000), 3).available_at(T0), 3);
        assert_eq!(core(10, 10, ms(1_000), 0).available_at(T0), 0);
    }

    #[test]
    fn test_core_size_is_one_cache_line() {
        #[cfg(not(feature = "metrics"))]
        assert_eq!(core::mem::size_of::<Core>(), 48);
        assert_eq!(core::mem::align_of::<BucketCore>(), 64);
        #[cfg(not(feature = "metrics"))]
        assert_eq!(core::mem::size_of::<BucketCore>(), 64);
    }

    #[test]
    fn test_core_config_round_trips_exactly() {
        for period in [
            Duration::from_nanos(1),
            Duration::new(3, 999_999_999),
            Duration::MAX,
        ] {
            let config = BucketConfig::new(u32::MAX, 7, period, 5).unwrap();
            assert_eq!(BucketCore::new(config, T0).config(), config);
        }
    }

    #[test]
    fn test_core_epoch_need_not_be_zero() {
        let config = BucketConfig::new(10, 10, ms(1_000), 0).unwrap();
        let start = Duration::from_secs(1_000_000);
        let bucket = BucketCore::new(config, start);
        assert_eq!(bucket.available_at(start), 0);
        assert_eq!(bucket.available_at(start + ms(300)), 3);
    }

    #[test]
    fn test_core_stale_now_never_refills() {
        let bucket = core(10, 10, ms(1_000), 10);
        assert!(bucket.try_acquire_at(10, ms(5_000))); // drain at t = 5 s
        // A reading from before the last commit sees an empty bucket, not a
        // 49.7-day gap.
        for stale in [T0, ms(1), ms(4_999)] {
            assert_eq!(bucket.available_at(stale), 0);
            assert!(!bucket.try_acquire_at(1, stale));
        }
        assert_eq!(bucket.available_at(ms(5_100)), 1);
    }

    #[test]
    fn test_core_denial_retry_after_is_exact_to_the_ms() {
        let bucket = core(3, 3, ms(7), 3);
        assert!(bucket.try_acquire_at(3, T0));
        assert_eq!(
            bucket.acquire_at(3, T0),
            Decision::Denied { retry_after: ms(7) }
        );
        assert!(bucket.try_acquire_at(3, ms(7)));
    }

    #[test]
    fn test_core_retry_after_rounds_sub_millisecond_up() {
        // 1 token per 1.5 ms: the 1.5 ms wait is reported as 2 ms, never 1.
        let bucket = core(1, 2, ms(3), 0);
        assert_eq!(
            bucket.acquire_at(1, T0),
            Decision::Denied { retry_after: ms(2) }
        );
        assert!(!bucket.try_acquire_at(1, Duration::from_micros(1_499)));
        assert!(bucket.try_acquire_at(1, Duration::from_micros(1_500)));
    }

    #[test]
    fn test_core_over_capacity_is_denied_forever() {
        let bucket = core(5, 5, ms(1_000), 5);
        assert_eq!(
            bucket.acquire_at(6, T0),
            Decision::Denied {
                retry_after: Duration::MAX
            }
        );
        assert_eq!(bucket.time_until_at(6, T0), None);
        assert_eq!(bucket.reserve_at(6, Duration::MAX, T0), None);
        assert_eq!(bucket.acquire_up_to_at(6, T0), 5);
    }

    #[test]
    fn test_core_zero_requests() {
        let bucket = core(1, 1, ms(1_000), 0);
        assert!(bucket.try_acquire_at(0, T0));
        assert_eq!(bucket.acquire_up_to_at(0, T0), 0);
        assert_eq!(bucket.time_until_at(0, T0), Some(Duration::ZERO));
        assert_eq!(
            bucket.reserve_at(0, Duration::ZERO, T0),
            Some(Duration::ZERO)
        );
        bucket.refund(0);
        assert_eq!(bucket.available_at(T0), 0);
    }

    #[test]
    fn test_core_large_capacity_is_not_clamped() {
        let bucket = core(u32::MAX, u32::MAX, ms(1_000), u32::MAX);
        assert_eq!(bucket.capacity(), u32::MAX);
        assert_eq!(bucket.available_at(T0), u32::MAX);
        assert!(bucket.try_acquire_at(u32::MAX, T0));
        assert_eq!(bucket.available_at(T0), 0);
        let ten_million = core(10_000_000, 10_000_000, ms(1_000), 10_000_000);
        assert!(ten_million.try_acquire_at(5_000_000, T0));
        assert_eq!(ten_million.available_at(T0), 5_000_000);
    }

    #[test]
    fn test_core_slow_rates_refill_on_time() {
        let day = Duration::from_secs(86_400);
        for (amount, period) in [
            (1, day),
            (1, day * 7),
            (1, day * 30),
            (1, day * 60),
            (10, day),
            (1, day * 365 * 100),
        ] {
            let bucket = core(1, amount, period, 0);
            let exact = period / amount;
            let hint = bucket.acquire_at(1, T0).retry_after().unwrap();
            assert!(hint >= exact, "{period:?}: hint {hint:?} < exact {exact:?}");
            assert!(
                hint <= exact + ms(1),
                "{period:?}: hint {hint:?} > {exact:?}"
            );
            assert!(!bucket.try_acquire_at(1, exact - Duration::from_nanos(1)));
            assert!(bucket.try_acquire_at(1, exact));
        }
    }

    #[test]
    fn test_core_frequent_successes_keep_fractional_refill() {
        // 1 token/hour with a big burst, drained 1/s for an hour: the refill
        // must survive 3600 intervening grants (1.0.0 lost all of it). The
        // bucket is full until the first grant at t = 1 s, so exactly one token
        // has accrued by t = 3601 s: 5000 − 3600 + 1.
        let bucket = core(5_000, 1, Duration::from_secs(3_600), 5_000);
        for second in 1..=3_600 {
            assert!(bucket.try_acquire_at(1, Duration::from_secs(second)));
        }
        assert_eq!(bucket.available_at(Duration::from_secs(3_600)), 1_400);
        assert_eq!(bucket.available_at(Duration::from_secs(3_601)), 1_401);
    }

    #[test]
    fn test_core_idle_gaps_across_old_wrap_boundaries() {
        for gap_ms in [
            (1_u64 << 31) - 1,
            1 << 31,
            (1 << 32) - 1,
            1 << 32,
            (1 << 32) + 5,
            1 << 40,
        ] {
            let bucket = core(10, 10, ms(1_000), 10);
            assert!(bucket.try_acquire_at(10, T0));
            assert_eq!(bucket.available_at(ms(gap_ms)), 10, "gap {gap_ms} ms");
        }
    }

    #[test]
    fn test_core_survives_centuries() {
        let bucket = core(10, 10, ms(1_000), 0);
        let century = Duration::from_secs(100 * 365 * 86_400);
        assert_eq!(bucket.available_at(century), 10);
        assert!(bucket.try_acquire_at(10, century));
        assert_eq!(bucket.available_at(century), 0);
    }

    /// Hammers one instant and returns the tokens granted and whether a
    /// zero-wait reservation of the capacity still succeeded afterwards.
    fn hammer_at(bucket: &BucketCore, now: Duration) -> (u32, bool) {
        let mut granted = 0_u32;
        for _ in 0..1_000 {
            if bucket.try_acquire_at(1, now) {
                granted += 1;
            }
            granted += bucket.acquire_up_to_at(1, now);
        }
        let reserved = bucket
            .reserve_at(bucket.capacity(), Duration::ZERO, now)
            .is_some();
        (granted, reserved)
    }

    #[test]
    fn test_core_saturated_clock_fails_closed() {
        // Rates near one unit per nanosecond saturate the unit clock before the
        // nanosecond reading does: 0.9 units/ns, 7/8 units/ns, and the fast
        // regime. Past the horizon the clock is frozen, so a bucket can hand
        // out what it holds and nothing more — never an unbounded stream.
        let year = 365 * 86_400;
        for (amount, period) in [
            (900_000_000, Duration::from_secs(1)),
            (7, Duration::from_nanos(8)),
            (u32::MAX, Duration::from_secs(1)),
            (1, Duration::from_secs(1)),
        ] {
            for now in [
                Duration::MAX,
                Duration::from_secs(u64::MAX),
                Duration::from_secs(450 * year),
                Duration::from_secs(500 * year),
            ] {
                let bucket = core(10, amount, period, 10);
                let (granted, reserved) = hammer_at(&bucket, now);
                assert!(
                    granted <= 10,
                    "{amount}/{period:?} at {now:?}: granted {granted}"
                );
                assert!(!reserved || granted == 0, "{amount}/{period:?} at {now:?}");
                assert!(bucket.available_at(now) <= 10);
                // At `Duration::MAX` every clock is frozen: once drained, the
                // wait is "never".
                if now == Duration::MAX && granted == 10 {
                    assert_eq!(bucket.time_until_at(1, now), None);
                    assert_eq!(bucket.acquire_at(1, now).retry_after(), Some(Duration::MAX));
                }
            }
        }
    }

    #[test]
    fn test_core_far_future_reading_never_mints_tokens_later() {
        // A garbage `now` past the horizon spends at most the capacity; later
        // sane readings see an empty bucket, never extra tokens.
        let bucket = core(5, 900_000_000, Duration::from_secs(1), 5);
        let (granted, _) = hammer_at(&bucket, Duration::MAX);
        assert!(granted <= 5);
        for sane in [T0, ms(1), Duration::from_secs(3_600)] {
            assert!(!bucket.try_acquire_at(1, sane));
            assert_eq!(bucket.acquire_up_to_at(5, sane), 0);
            assert!(bucket.reserve_at(1, Duration::MAX, sane).is_none());
        }
    }

    #[test]
    fn test_core_unreachable_wait_reports_never() {
        // One token per ~5.8e11 years: the wait outlives the clock.
        let bucket = core(1, 1, Duration::MAX, 1);
        assert!(bucket.try_acquire_at(1, T0));
        assert_eq!(
            bucket.acquire_at(1, T0),
            Decision::Denied {
                retry_after: Duration::MAX
            }
        );
        assert_eq!(bucket.time_until_at(1, T0), None);
        assert_eq!(bucket.reserve_at(1, Duration::MAX, T0), None);
    }

    #[test]
    fn test_core_wait_past_the_clock_horizon_reports_never() {
        // One token per 600 years: due after the ~584-year reading limit.
        let bucket = core(1, 1, Duration::from_secs(600 * 365 * 86_400), 0);
        assert_eq!(bucket.time_until_at(1, T0), None);
        assert_eq!(bucket.acquire_at(1, T0).retry_after(), Some(Duration::MAX));
        // One per 500 years is due within the horizon.
        let bucket = core(1, 1, Duration::from_secs(500 * 365 * 86_400), 0);
        assert_eq!(
            bucket.time_until_at(1, T0),
            Some(Duration::from_secs(500 * 365 * 86_400))
        );
    }

    #[test]
    fn test_core_concurrent_mixed_operations_conserve_tokens() {
        use std::sync::Arc;
        use std::thread;
        use std::vec::Vec;

        // Frozen time, every mutating operation racing: what the threads still
        // hold plus what is available must equal the initial fill exactly.
        let bucket = Arc::new(core(10_000, 1, ms(1_000), 10_000));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let bucket = Arc::clone(&bucket);
                thread::spawn(move || {
                    let mut held = 0_u64;
                    for round in 0..2_000_u32 {
                        let n = 1 + (round + i) % 5;
                        match round % 4 {
                            0 => held += u64::from(bucket.acquire_up_to_at(n, T0)),
                            1 => {
                                if bucket.reserve_at(n, Duration::ZERO, T0).is_some() {
                                    held += u64::from(n);
                                }
                            }
                            2 => {
                                if bucket.try_acquire_at(n, T0) {
                                    held += u64::from(n);
                                }
                            }
                            _ => {
                                let back = u32::try_from(held.min(u64::from(n))).unwrap();
                                bucket.refund(back);
                                held -= u64::from(back);
                            }
                        }
                    }
                    held
                })
            })
            .collect();
        let held: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(held + u64::from(bucket.available_at(T0)), 10_000);
    }

    #[test]
    fn test_core_extreme_fast_rate_does_not_overflow() {
        // ~4.3 × 10^18 tokens/s: a unit is 2^30 tokens, so a grant rounds up to
        // a whole unit, but nothing overflows and the bucket refills within two
        // nanoseconds.
        let bucket = core(u32::MAX, u32::MAX, Duration::from_nanos(1), u32::MAX);
        let two_ns = Duration::from_nanos(2);
        assert!(bucket.try_acquire_at(u32::MAX, T0));
        assert_eq!(bucket.available_at(T0), 0);
        assert!(bucket.available_at(Duration::from_nanos(1)) >= 3 << 30);
        assert_eq!(bucket.available_at(two_ns), u32::MAX);
        assert!(bucket.try_acquire_at(1, two_ns));
        // One token cost a whole 2^30-token unit.
        assert_eq!(bucket.available_at(two_ns), u32::MAX - (1 << 30) + 1);
        assert!(bucket.try_acquire_at(1, Duration::MAX));
    }

    #[test]
    fn test_core_fast_per_second_rates_charge_exactly_one_token() {
        // 10^9 to u32::MAX tokens per second: every grant costs exactly what it
        // asks for, refill is exact, and the clock still lasts a century.
        for rate in [1_000_000_000_u32, 2_000_000_000, 3_000_000_000, u32::MAX] {
            let bucket = core(rate, rate, Duration::from_secs(1), rate);
            assert!(bucket.try_acquire_at(1, T0));
            assert_eq!(bucket.available_at(T0), rate - 1, "{rate}/s");
            let mut drained = 1_u64;
            while bucket.try_acquire_at(1_000_000, T0) {
                drained += 1_000_000;
            }
            drained += u64::from(bucket.acquire_up_to_at(u32::MAX, T0));
            assert_eq!(drained, u64::from(rate), "{rate}/s conserves");
            // One millisecond accrues exactly rate / 1000 tokens.
            assert_eq!(
                u64::from(bucket.available_at(ms(1))),
                u64::from(rate) / 1_000
            );
            let century = Duration::from_secs(100 * 365 * 86_400);
            assert_eq!(bucket.available_at(century), rate);
        }
        // Ten tokens at 3 × 10^9/s: a full bucket grants ten single tokens.
        let small = core(10, 3_000_000_000, Duration::from_secs(1), 10);
        let granted = (0..20).filter(|_| small.try_acquire_at(1, T0)).count();
        assert_eq!(granted, 10);
    }

    #[test]
    fn test_core_acquire_up_to_takes_what_is_there() {
        let bucket = core(100, 100, ms(1_000), 30);
        assert_eq!(bucket.acquire_up_to_at(10, T0), 10);
        assert_eq!(bucket.acquire_up_to_at(50, T0), 20);
        assert_eq!(bucket.acquire_up_to_at(50, T0), 0);
        assert_eq!(bucket.acquire_up_to_at(50, ms(250)), 25);
        assert_eq!(bucket.acquire_up_to_at(500, ms(10_000)), 100);
    }

    #[test]
    fn test_core_reserve_queues_and_blocks_later_acquires() {
        let bucket = core(10, 10, ms(1_000), 10);
        let max = Duration::from_secs(60);
        assert_eq!(bucket.reserve_at(4, max, T0), Some(Duration::ZERO));
        assert_eq!(bucket.reserve_at(10, max, T0), Some(ms(400)));
        // In debt: nothing available, and acquires wait behind the reservation.
        assert_eq!(bucket.available_at(T0), 0);
        assert_eq!(bucket.time_until_at(1, T0), Some(ms(500)));
        assert!(!bucket.try_acquire_at(1, ms(400)));
        assert!(bucket.try_acquire_at(1, ms(500)));
        // A reservation over the limit takes nothing.
        assert_eq!(bucket.reserve_at(10, ms(999), ms(500)), None);
        assert_eq!(bucket.reserve_at(10, ms(1_000), ms(500)), Some(ms(1_000)));
    }

    #[test]
    fn test_core_refund_restores_and_caps() {
        let bucket = core(10, 10, ms(1_000), 10);
        assert!(bucket.try_acquire_at(10, T0));
        bucket.refund(4);
        assert_eq!(bucket.available_at(T0), 4);
        bucket.refund(u32::MAX);
        assert_eq!(bucket.available_at(T0), 10);
        assert!(!bucket.try_acquire_at(11, T0));
        // Refund pays down a reservation's debt.
        assert_eq!(
            bucket.reserve_at(10, Duration::MAX, T0),
            Some(Duration::ZERO)
        );
        assert_eq!(bucket.reserve_at(10, Duration::MAX, T0), Some(ms(1_000)));
        bucket.refund(10);
        assert_eq!(bucket.available_at(T0), 0);
        assert_eq!(bucket.time_until_at(1, T0), Some(ms(100)));
    }

    #[test]
    fn test_core_reset_cancels_debt() {
        let bucket = core(10, 10, ms(1_000), 0);
        assert!(bucket.reserve_at(10, Duration::MAX, T0).is_some());
        bucket.reset_at(T0);
        assert_eq!(bucket.available_at(T0), 10);
    }

    #[test]
    fn test_core_degenerate_config_grants_nothing() {
        let bucket = BucketCore::new(BucketConfig::raw(0, 0, ms(1_000), 0), T0);
        assert_eq!(bucket.capacity(), 0);
        assert_eq!(bucket.available_at(ms(5_000)), 0);
        assert!(!bucket.try_acquire_at(1, ms(5_000)));
        assert!(bucket.try_acquire_at(0, ms(5_000)));
        assert_eq!(bucket.acquire_up_to_at(5, ms(5_000)), 0);
        bucket.refund(5);
        bucket.reset_at(ms(5_000));
        assert_eq!(bucket.available_at(ms(5_000)), 0);
    }

    #[test]
    fn test_core_debug_names_fields() {
        let rendered = format!("{:?}", core(3, 3, ms(1_000), 3));
        assert!(rendered.starts_with("BucketCore"));
        assert!(rendered.contains("capacity: 3"));
    }

    #[test]
    fn test_rate_fits_the_word_for_every_extreme() {
        let periods = [
            Duration::from_nanos(1),
            Duration::from_nanos(7),
            ms(1),
            Duration::from_secs(1),
            Duration::from_secs(86_400 * 365 * 37),
            Duration::from_secs(u64::MAX / 2),
            Duration::MAX,
        ];
        for &capacity in &[1_u32, 3, 1_000, 4_294_967, u32::MAX] {
            for &amount in &[1_u32, 7, 1_000_000, u32::MAX] {
                for &period in &periods {
                    let rate = Scale::derive(capacity, amount, period);
                    assert!(rate.rate_int > 0 || rate.rate_frac > 0);
                    assert!(rate.rate_int <= 4);
                    assert!((-32..=60).contains(&rate.unit_shift));
                    let config = BucketConfig::new(capacity, amount, period, capacity).unwrap();
                    let core = Core::new(config, T0);
                    let burst = core.burst();
                    assert!(burst <= 1 << BURST_BITS.unsigned_abs());
                    assert!(burst < ORIGIN);
                    // The clock never runs faster than 4.3 units per ns (one
                    // below 10^9 tokens/s), so it lasts at least ~100 years
                    // (~438) before saturating.
                    let one_sec = core.units(Duration::from_secs(1)) - ORIGIN;
                    assert!(one_sec <= 4_300_000_001, "{one_sec}");
                    if u128::from(amount) * 1_000_000_000 < period.as_nanos() * 1_000_000_000 {
                        // Below one token per ns.
                        assert!(one_sec <= 1_000_000_000, "{one_sec}");
                    }
                    // A full bucket grants exactly its capacity.
                    assert_eq!(core.available(T0), capacity);
                    assert_eq!(core.tokens_for(burst), capacity);
                    assert!(core.acquire(capacity, T0).is_allowed());
                    assert_eq!(core.available(T0), 0);
                }
            }
        }
    }

    #[test]
    fn test_available_agrees_with_acquire_exactly() {
        // available() must be the exact grant boundary: n is grantable, n + 1 is
        // not, at the same instant.
        for &(capacity, amount, period_ns) in &[
            (10_u32, 10_u32, 1_000_000_000_u64),
            (7, 3, 7_000_000),
            (1_000, 1, 3_600_000_000_000),
            (u32::MAX, u32::MAX, 1),
            (1_234_567, 89, 1_000_003),
        ] {
            let config =
                BucketConfig::new(capacity, amount, Duration::from_nanos(period_ns), 0).unwrap();
            for step in [1_u64, 999, 1_000_001, 333_333_333, 7_000_000_000] {
                let a = BucketCore::new(config, T0);
                let b = BucketCore::new(config, T0);
                let now = Duration::from_nanos(step);
                let n = a.available_at(now);
                if n < capacity {
                    assert!(!b.try_acquire_at(n + 1, now));
                }
                assert!(b.try_acquire_at(n, now), "cap {capacity} step {step} n {n}");
            }
        }
    }
}
