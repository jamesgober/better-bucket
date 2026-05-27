//! The single token bucket and the `TokenBucket` trait.
//!
//! This is the lock-free core. All mutable state lives in one `AtomicU64` that
//! packs the current token count and the time of the last refill; `try_acquire`
//! is a single `compare_exchange_weak` loop with lazy refill computed from the
//! injected monotonic clock. There is no lock and no allocation on the acquire
//! path, and the bucket is cache-line aligned so independent buckets never
//! falsely share. The public surface is identical to the `0.2` foundation
//! release — only the internals changed.
//!
//! # Packing
//!
//! The state word is split:
//! - **upper 32 bits** — tokens in *millitokens* (thousandths of a token), for
//!   sub-token refill resolution. Capped at [`u32::MAX`] millitokens, so the
//!   effective capacity ceiling is about 4.29 million tokens.
//! - **lower 32 bits** — milliseconds since the bucket's `created_at` anchor of
//!   the last refill computation.
//!
//! The millisecond field saturates after ~49.7 days of clock advance, after
//! which refill stalls; [`Bucket::reset`] re-anchors it for processes that run
//! longer than that between resets.

use core::time::Duration;

#[cfg(loom)]
use loom::sync::atomic::{AtomicU64, Ordering};

use clock_lib::{Clock, Monotonic, SystemClock};
#[cfg(not(loom))]
use core::sync::atomic::{AtomicU64, Ordering};

use crate::config::BucketConfig;
use crate::decision::Decision;

/// Millitokens per whole token.
const MILLI: u64 = 1_000;

/// Packs `millitokens` (clamped to 32 bits) and `last_ms` into one word.
#[inline]
fn pack(millitokens: u64, last_ms: u32) -> u64 {
    (millitokens.min(u64::from(u32::MAX)) << 32) | u64::from(last_ms)
}

/// Unpacks the state word into `(millitokens, last_ms)`.
#[inline]
fn unpack(state: u64) -> (u64, u32) {
    (state >> 32, (state & u64::from(u32::MAX)) as u32)
}

/// Maps a Tier-1 request to a config, collapsing a degenerate request (zero
/// capacity, amount, or period) to a bucket that grants nothing. That is
/// well-defined and safe rather than a panic; [`BucketConfig::new`] is the
/// validated, error-returning path.
fn tier1_config(capacity: u32, amount: u32, period: Duration, initial: u32) -> BucketConfig {
    if capacity == 0 || amount == 0 || period.is_zero() {
        BucketConfig::raw(0, 0, Duration::from_secs(1), 0)
    } else {
        BucketConfig::raw(capacity, amount, period, initial)
    }
}

/// A token bucket: a counter that refills over time and grants tokens on demand.
///
/// A bucket holds up to its capacity in tokens, accrues more at a fixed rate,
/// and hands them out when asked. The hot path is **lock-free** — a single
/// `compare_exchange_weak` on a packed atomic word — and **allocation-free**.
/// Refill is **lazy**: there is no background thread or timer, the token count
/// is brought current from the monotonic clock the instant you call
/// [`acquire`](Self::acquire), [`try_acquire`](Self::try_acquire), or
/// [`available`](Self::available).
///
/// The type parameter `C` is the time source. It defaults to
/// [`SystemClock`](clock_lib::SystemClock) (the OS monotonic clock); inject a
/// [`ManualClock`](clock_lib::ManualClock) with [`with_clock`](Self::with_clock)
/// to drive time by hand in tests. `Bucket` is `Send + Sync` whenever its clock
/// is, which every [`Clock`] implementation guarantees.
///
/// # Limits
///
/// The packed representation caps capacity at about 4.29 million tokens
/// (`u32::MAX` millitokens) and tracks time in milliseconds relative to
/// construction; that millisecond counter saturates after ~49.7 days of clock
/// advance, stalling refill until [`reset`](Self::reset) re-anchors it.
///
/// # Examples
///
/// The one-line common case:
///
/// ```
/// use better_bucket::Bucket;
///
/// let bucket = Bucket::per_second(100);
/// if bucket.try_acquire(1) {
///     // allowed — do the work
/// }
/// ```
#[repr(align(64))]
pub struct Bucket<C: Clock = SystemClock> {
    /// Packed `(millitokens << 32) | last_ms`. The only mutable state, and the
    /// single point of synchronisation.
    state: AtomicU64,
    /// Capacity in millitokens, already clamped to the 32-bit packing ceiling.
    capacity_millitokens: u64,
    /// Refill numerator: `refill_amount * 1_000_000_000`. Zero means no refill.
    /// Paired with `period_nanos`, `elapsed_ms * refill_numerator / period_nanos`
    /// yields the millitokens accrued — exact integer math that keeps
    /// sub-millisecond periods working despite the millisecond tick.
    refill_numerator: u64,
    /// Refill period in nanoseconds. Zero means no refill.
    period_nanos: u64,
    /// The monotonic anchor that `last_ms` is measured from.
    created_at: Monotonic,
    /// The original configuration, kept for [`config`](Self::config).
    config: BucketConfig,
    /// The injected time source.
    clock: C,
}

/// Constructs a bucket from a finished config and a clock, anchoring the refill
/// clock at the supplied clock's current reading and filling to `initial`.
fn build<C: Clock>(config: BucketConfig, clock: C) -> Bucket<C> {
    let created_at = clock.now();
    let capacity_millitokens = u64::from(config.capacity())
        .saturating_mul(MILLI)
        .min(u64::from(u32::MAX));
    // `refill_amount * 1_000` millitokens accrue per `refill_period`; scaling by
    // a further 1_000_000 lets us divide by the period in *nanoseconds*, which
    // keeps sub-millisecond periods exact. `0` disables refill.
    let (refill_numerator, period_nanos) = if config.refill_amount() == 0 {
        (0, 0)
    } else {
        let numerator = u64::from(config.refill_amount()).saturating_mul(1_000_000_000);
        let period = u64::try_from(config.refill_period().as_nanos()).unwrap_or(u64::MAX);
        (numerator, period)
    };
    let initial_millitokens = (u64::from(config.initial()) * MILLI).min(capacity_millitokens);
    Bucket {
        state: AtomicU64::new(pack(initial_millitokens, 0)),
        capacity_millitokens,
        refill_numerator,
        period_nanos,
        created_at,
        config,
        clock,
    }
}

impl Bucket<SystemClock> {
    /// Creates a bucket of capacity `rate` that refills `rate` tokens per
    /// second, starting full, driven by the OS monotonic clock.
    ///
    /// This is the headline Tier-1 constructor. A `rate` of `0` yields a bucket
    /// that grants nothing (capacity `0`); use [`BucketConfig::new`] when you
    /// want zero rejected as an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    ///
    /// let bucket = Bucket::per_second(50);
    /// assert_eq!(bucket.capacity(), 50);
    /// assert!(bucket.try_acquire(1));
    /// ```
    #[must_use]
    pub fn per_second(rate: u32) -> Self {
        Self::from_config(tier1_config(rate, rate, Duration::from_secs(1), rate))
    }

    /// Creates a bucket of capacity `amount` that refills `amount` tokens every
    /// `period`, starting full, driven by the OS monotonic clock.
    ///
    /// Use this when the natural rate is not per-second — e.g. 5 tokens per 100
    /// milliseconds, or 1000 per minute. An `amount` of `0` or a zero `period`
    /// yields a bucket that grants nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    /// use std::time::Duration;
    ///
    /// // 5 tokens every 100ms.
    /// let bucket = Bucket::per_duration(5, Duration::from_millis(100));
    /// assert_eq!(bucket.capacity(), 5);
    /// ```
    #[must_use]
    pub fn per_duration(amount: u32, period: Duration) -> Self {
        Self::from_config(tier1_config(amount, amount, period, amount))
    }

    /// Creates a bucket from a validated [`BucketConfig`], driven by the OS
    /// monotonic clock.
    ///
    /// Use this when you need full control over capacity, rate, and initial
    /// fill independently (e.g. a large burst ceiling with a slow refill, or a
    /// bucket that starts empty).
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{Bucket, BucketConfig};
    /// use std::time::Duration;
    ///
    /// // 500-token burst, 100/sec refill, starting empty.
    /// let config = BucketConfig::new(500, 100, Duration::from_secs(1), 0)?;
    /// let bucket = Bucket::from_config(config);
    /// assert_eq!(bucket.available(), 0);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[must_use]
    pub fn from_config(config: BucketConfig) -> Self {
        build(config, SystemClock::new())
    }
}

impl<C: Clock> Bucket<C> {
    /// Replaces the bucket's time source, resetting it to its initial fill
    /// anchored at the new clock's current reading.
    ///
    /// This is the clock-injection seam. The intended use is immediately after
    /// construction — chiefly in tests, where injecting a
    /// [`ManualClock`](clock_lib::ManualClock) makes refill behaviour
    /// deterministic with no `sleep`.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    /// use clock_lib::ManualClock;
    /// use std::sync::Arc;
    /// use std::time::Duration;
    ///
    /// let clock = Arc::new(ManualClock::new());
    /// let bucket = Bucket::per_second(10).with_clock(Arc::clone(&clock));
    ///
    /// assert!(bucket.try_acquire(10)); // drain it
    /// assert!(!bucket.try_acquire(1)); // empty
    ///
    /// clock.advance(Duration::from_secs(1)); // no real sleep
    /// assert_eq!(bucket.available(), 10);  // fully refilled
    /// ```
    #[must_use]
    pub fn with_clock<C2: Clock>(self, clock: C2) -> Bucket<C2> {
        build(self.config, clock)
    }

    /// Attempts to take `n` tokens, returning the full [`Decision`].
    ///
    /// Brings the bucket current (lazy refill) and, if at least `n` tokens are
    /// available, deducts them and returns [`Decision::Allowed`]. Otherwise the
    /// bucket is left untouched and [`Decision::Denied`] carries the minimum
    /// wait until the request would succeed. Requesting `0` always succeeds;
    /// requesting more than the capacity can never succeed (the denial's
    /// `retry_after` is [`Duration::MAX`]).
    ///
    /// This never blocks and never allocates.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{Bucket, Decision};
    ///
    /// let bucket = Bucket::per_second(5);
    /// assert_eq!(bucket.acquire(3), Decision::Allowed);
    /// assert_eq!(bucket.available(), 2);
    /// ```
    pub fn acquire(&self, n: u32) -> Decision {
        self.acquire_inner(n)
    }

    /// Attempts to take `n` tokens, returning whether it succeeded.
    ///
    /// The one-line convenience over [`acquire`](Self::acquire): equivalent to
    /// `self.acquire(n).is_allowed()`, for the common case where you only need
    /// allow/deny and not the retry hint.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    ///
    /// let bucket = Bucket::per_second(1);
    /// assert!(bucket.try_acquire(1));
    /// assert!(!bucket.try_acquire(1)); // drained
    /// ```
    #[must_use]
    pub fn try_acquire(&self, n: u32) -> bool {
        self.acquire_inner(n).is_allowed()
    }

    /// Returns how many whole tokens are available right now, after lazy refill.
    ///
    /// This is a momentary snapshot; under concurrent acquires it can be stale
    /// the instant it returns. Treat it as advisory.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    ///
    /// let bucket = Bucket::per_second(10);
    /// assert_eq!(bucket.available(), 10);
    /// assert!(bucket.try_acquire(4));
    /// assert_eq!(bucket.available(), 6);
    /// ```
    #[must_use]
    pub fn available(&self) -> u32 {
        let now_ms = self.now_ms();
        let (tokens_mt, last_ms) = unpack(self.state.load(Ordering::Relaxed));
        let refilled = self.refilled(tokens_mt, last_ms, now_ms);
        u32::try_from(refilled / MILLI).unwrap_or(u32::MAX)
    }

    /// Returns the bucket's capacity (its burst ceiling), in whole tokens.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    ///
    /// assert_eq!(Bucket::per_second(64).capacity(), 64);
    /// ```
    #[must_use]
    pub const fn capacity(&self) -> u32 {
        (self.capacity_millitokens / MILLI) as u32
    }

    /// Returns the configuration this bucket was built from.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    /// use std::time::Duration;
    ///
    /// let bucket = Bucket::per_second(10);
    /// assert_eq!(bucket.config().refill_period(), Duration::from_secs(1));
    /// ```
    #[must_use]
    pub const fn config(&self) -> BucketConfig {
        self.config
    }

    /// Refills the bucket to full and re-anchors its internal clock to now.
    ///
    /// Two uses: discard accumulated debt to grant a fresh burst, and re-anchor
    /// the internal millisecond counter on a process that runs longer than the
    /// ~49.7-day saturation window (call `reset` periodically to keep refill
    /// from stalling).
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    ///
    /// let bucket = Bucket::per_second(4);
    /// assert!(bucket.try_acquire(4));
    /// assert_eq!(bucket.available(), 0);
    /// bucket.reset();
    /// assert_eq!(bucket.available(), 4);
    /// ```
    pub fn reset(&self) {
        let now_ms = self.now_ms();
        self.state
            .store(pack(self.capacity_millitokens, now_ms), Ordering::Relaxed);
    }

    /// Milliseconds since `created_at`, saturating into the 32-bit time field.
    #[inline]
    fn now_ms(&self) -> u32 {
        let elapsed = self.clock.now().saturating_duration_since(self.created_at);
        u32::try_from(elapsed.as_millis().min(u128::from(u32::MAX))).unwrap_or(u32::MAX)
    }

    /// The millitoken count after refilling over `last_ms → now_ms`, capped at
    /// capacity. Saturating throughout: a huge elapsed gap fills to capacity, it
    /// can never wrap or overflow.
    #[inline]
    fn refilled(&self, tokens_mt: u64, last_ms: u32, now_ms: u32) -> u64 {
        if self.refill_numerator == 0 || self.period_nanos == 0 {
            return tokens_mt;
        }
        let elapsed_ms = u64::from(now_ms.saturating_sub(last_ms));
        let added = u128::from(elapsed_ms).saturating_mul(u128::from(self.refill_numerator))
            / u128::from(self.period_nanos);
        let added_mt = u64::try_from(added).unwrap_or(u64::MAX);
        tokens_mt
            .saturating_add(added_mt)
            .min(self.capacity_millitokens)
    }

    /// The minimum time for `deficit_mt` millitokens to accrue, rounded up.
    /// [`Duration::MAX`] if the bucket never refills.
    fn time_for(&self, deficit_mt: u64) -> Duration {
        if self.refill_numerator == 0 || self.period_nanos == 0 {
            return Duration::MAX;
        }
        let numerator = u128::from(deficit_mt)
            .saturating_mul(u128::from(self.period_nanos))
            .saturating_add(u128::from(self.refill_numerator) - 1);
        let millis = numerator / u128::from(self.refill_numerator);
        Duration::from_millis(u64::try_from(millis).unwrap_or(u64::MAX))
    }

    fn acquire_inner(&self, n: u32) -> Decision {
        if n == 0 {
            // Zero tokens are always available, even from an empty bucket.
            return Decision::Allowed;
        }
        let need_mt = u64::from(n) * MILLI;
        if need_mt > self.capacity_millitokens {
            // More than the bucket can ever hold: it can never be granted.
            return Decision::Denied {
                retry_after: Duration::MAX,
            };
        }

        let now_ms = self.now_ms();
        loop {
            let current = self.state.load(Ordering::Relaxed);
            let (tokens_mt, last_ms) = unpack(current);
            let refilled = self.refilled(tokens_mt, last_ms, now_ms);
            if refilled < need_mt {
                // Denied: report the wait for the shortfall to accrue. No write,
                // so a denied request never contends with a granting one.
                return Decision::Denied {
                    retry_after: self.time_for(need_mt - refilled),
                };
            }
            let next = pack(refilled - need_mt, now_ms);
            // Relaxed is sufficient: the only shared state is this word, and the
            // CAS gives the read-modify-write atomicity the no-over-grant
            // contract depends on. A spurious or lost race retries.
            if self
                .state
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return Decision::Allowed;
            }
        }
    }
}

impl<C: Clock> core::fmt::Debug for Bucket<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Bucket")
            .field("capacity", &self.capacity())
            .field("available", &self.available())
            .field("config", &self.config)
            .finish()
    }
}

/// The token-bucket surface a consumer depends on.
///
/// `TokenBucket` is the abstraction `rate-net` (and any other consumer) codes
/// against, so it can hold a bucket without naming its concrete clock type. It
/// mirrors the inherent methods of [`Bucket`]; see those for the detailed
/// contract of each.
pub trait TokenBucket {
    /// Attempts to take `n` tokens, returning the full [`Decision`].
    fn acquire(&self, n: u32) -> Decision;

    /// Attempts to take `n` tokens, returning whether it succeeded.
    #[must_use]
    fn try_acquire(&self, n: u32) -> bool;

    /// Returns the whole tokens available right now, after lazy refill.
    #[must_use]
    fn available(&self) -> u32;

    /// Returns the bucket's capacity (its burst ceiling).
    #[must_use]
    fn capacity(&self) -> u32;
}

impl<C: Clock> TokenBucket for Bucket<C> {
    fn acquire(&self, n: u32) -> Decision {
        self.acquire_inner(n)
    }

    fn try_acquire(&self, n: u32) -> bool {
        self.acquire_inner(n).is_allowed()
    }

    fn available(&self) -> u32 {
        Bucket::available(self)
    }

    fn capacity(&self) -> u32 {
        self.capacity()
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{Bucket, TokenBucket};
    use crate::decision::Decision;
    use clock_lib::{ManualClock, SystemClock};
    use core::time::Duration;
    use std::sync::Arc;
    use std::thread;

    /// A bucket driven by a `ManualClock` the test controls, so refill is
    /// deterministic with no real time passing.
    fn manual_bucket(rate: u32) -> (Arc<ManualClock>, Bucket<Arc<ManualClock>>) {
        let clock = Arc::new(ManualClock::new());
        let bucket = Bucket::per_second(rate).with_clock(Arc::clone(&clock));
        (clock, bucket)
    }

    #[test]
    fn test_bucket_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Bucket<SystemClock>>();
        assert_send_sync::<Bucket<Arc<ManualClock>>>();
    }

    #[test]
    fn test_starts_full() {
        let (_clock, bucket) = manual_bucket(10);
        assert_eq!(bucket.available(), 10);
        assert_eq!(bucket.capacity(), 10);
    }

    #[test]
    fn test_acquire_deducts_tokens() {
        let (_clock, bucket) = manual_bucket(10);
        assert_eq!(bucket.acquire(3), Decision::Allowed);
        assert_eq!(bucket.available(), 7);
    }

    #[test]
    fn test_exact_empty_then_denied() {
        let (_clock, bucket) = manual_bucket(10);
        assert!(bucket.try_acquire(10)); // exact-empty
        assert_eq!(bucket.available(), 0);
        assert!(!bucket.try_acquire(1));
    }

    #[test]
    fn test_acquire_zero_always_allowed() {
        let (_clock, bucket) = manual_bucket(1);
        assert!(bucket.try_acquire(1)); // drain
        assert!(!bucket.try_acquire(1));
        assert!(bucket.try_acquire(0)); // still allowed when empty
    }

    #[test]
    fn test_request_above_capacity_never_grantable() {
        let (_clock, bucket) = manual_bucket(5);
        assert_eq!(
            bucket.acquire(6),
            Decision::Denied {
                retry_after: Duration::MAX
            }
        );
    }

    #[test]
    fn test_full_refill_after_one_period() {
        let (clock, bucket) = manual_bucket(10);
        assert!(bucket.try_acquire(10));
        assert!(!bucket.try_acquire(1));
        clock.advance(Duration::from_secs(1));
        assert_eq!(bucket.available(), 10);
        assert!(bucket.try_acquire(10));
    }

    #[test]
    fn test_partial_refill_is_proportional() {
        let (clock, bucket) = manual_bucket(100);
        assert!(bucket.try_acquire(100));
        clock.advance(Duration::from_millis(250)); // a quarter second
        assert_eq!(bucket.available(), 25);
    }

    #[test]
    fn test_refill_saturates_at_capacity() {
        let (clock, bucket) = manual_bucket(10);
        assert!(bucket.try_acquire(10));
        clock.advance(Duration::from_secs(100)); // would be 1000 tokens
        assert_eq!(bucket.available(), 10); // clamped to capacity
    }

    #[test]
    fn test_denied_reports_retry_after() {
        let (_clock, bucket) = manual_bucket(10);
        assert!(bucket.try_acquire(10)); // empty
        // Five tokens at 10/sec accrue in 500ms.
        assert_eq!(
            bucket.acquire(5),
            Decision::Denied {
                retry_after: Duration::from_millis(500)
            }
        );
    }

    #[test]
    fn test_per_duration_uses_custom_period() {
        let clock = Arc::new(ManualClock::new());
        let bucket =
            Bucket::per_duration(5, Duration::from_millis(100)).with_clock(Arc::clone(&clock));
        assert!(bucket.try_acquire(5));
        clock.advance(Duration::from_millis(100));
        assert_eq!(bucket.available(), 5);
    }

    #[test]
    fn test_sub_millisecond_period_still_refills() {
        // 5 tokens per 200µs ⇒ 25 tokens/ms. The millisecond tick is coarse but
        // the rate is computed from nanoseconds, so a full ms refills fully.
        let clock = Arc::new(ManualClock::new());
        let bucket =
            Bucket::per_duration(5, Duration::from_micros(200)).with_clock(Arc::clone(&clock));
        assert!(bucket.try_acquire(5));
        clock.advance(Duration::from_millis(1));
        assert_eq!(bucket.available(), 5); // capped at capacity
    }

    #[test]
    fn test_zero_rate_is_deny_all() {
        let bucket = Bucket::per_second(0);
        assert_eq!(bucket.capacity(), 0);
        assert_eq!(bucket.available(), 0);
        assert!(!bucket.try_acquire(1));
        assert!(bucket.try_acquire(0));
    }

    #[test]
    fn test_reset_refills_to_capacity() {
        let (_clock, bucket) = manual_bucket(5);
        assert!(bucket.try_acquire(5));
        assert_eq!(bucket.available(), 0);
        bucket.reset();
        assert_eq!(bucket.available(), 5);
    }

    #[test]
    fn test_trait_object_safe_surface() {
        let (_clock, bucket) = manual_bucket(4);
        let as_trait: &dyn TokenBucket = &bucket;
        assert_eq!(as_trait.capacity(), 4);
        assert!(as_trait.try_acquire(4));
        assert!(!as_trait.try_acquire(1));
    }

    #[test]
    fn test_concurrent_acquire_never_over_grants() {
        // 100 tokens, no refill (clock never advances). Eight threads each
        // demand 30 — total demand 240, available 100. Under a correct CAS,
        // exactly 100 succeed: no over-grant (≤ 100) and no lost token (= 100).
        let clock = Arc::new(ManualClock::new());
        let bucket = Arc::new(Bucket::per_second(100).with_clock(clock));
        let threads = 8;
        let demand = 30u32;

        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let bucket = Arc::clone(&bucket);
                thread::spawn(move || {
                    let mut taken = 0u32;
                    for _ in 0..demand {
                        if bucket.try_acquire(1) {
                            taken += 1;
                        }
                    }
                    taken
                })
            })
            .collect();

        let total: u32 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(total, 100, "CAS bucket must grant exactly capacity");
        assert_eq!(bucket.available(), 0);
    }

    #[test]
    fn test_pack_unpack_round_trip() {
        for &mt in &[0_u64, 1, 1_000, 50_000, u64::from(u32::MAX)] {
            for &ms in &[0_u32, 1, 1_000, u32::MAX] {
                let (got_mt, got_ms) = super::unpack(super::pack(mt, ms));
                assert_eq!(got_mt, mt.min(u64::from(u32::MAX)));
                assert_eq!(got_ms, ms);
            }
        }
    }
}
