//! The clock-driven token bucket and the `TokenBucket` trait.
//!
//! [`Bucket`] is the lock-free engine (see `engine.rs`) plus an injected
//! monotonic clock and the instant it was built. Every method that needs the
//! time reads the clock once and hands the elapsed time to the engine; every
//! `*_at` twin takes the reading from the caller instead, so one clock read can
//! serve many buckets and tests can drive time exactly.
//!
//! All mutable state is one `AtomicU64` holding the instant the bucket would
//! have been empty, in nanoseconds. A grant is a single `compare_exchange_weak`
//! that moves that instant forward; a stale or backward clock reading can only
//! see fewer tokens, never more. Time never wraps (the range is ~438 years) and
//! capacity is exact up to `u32::MAX`.

use core::time::Duration;

use clock_lib::{Clock, Monotonic, SystemClock};

use crate::config::BucketConfig;
use crate::decision::Decision;
use crate::engine::Core;

/// Maps a Tier-1 request to a config, collapsing a degenerate request (zero
/// capacity, amount, or period) to a bucket that grants nothing. That is
/// well-defined and safe rather than a panic; [`BucketConfig::new`] is the
/// validated, error-returning path.
const fn tier1_config(capacity: u32, amount: u32, period: Duration, initial: u32) -> BucketConfig {
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
/// `compare_exchange_weak` on one atomic word — and **allocation-free**.
/// Refill is **lazy**: there is no background thread or timer, the token count
/// is brought current from the monotonic clock the instant you call
/// [`acquire`](Self::acquire), [`try_acquire`](Self::try_acquire), or
/// [`available`](Self::available).
///
/// The type parameter `C` is the time source. It defaults to
/// [`SystemClock`](clock_lib::SystemClock) (the OS monotonic clock); inject a
/// [`ManualClock`](clock_lib::ManualClock) with
/// [`with_config_and_clock`](Self::with_config_and_clock) or
/// [`with_clock`](Self::with_clock) to drive time by hand in tests. `Bucket` is
/// `Send + Sync` whenever its clock is, which every [`Clock`] implementation
/// guarantees.
///
/// # Guarantees
///
/// Over any window of time, under any interleaving of concurrent callers, the
/// tokens granted never exceed the capacity plus what the configured rate
/// accrues over that window. The only slack is the bucket's resolution: a token
/// may be granted a few nanoseconds before it is exactly due. A stale or
/// non-monotonic clock reading can only cause a denial, never an extra grant.
/// [`reset`](Self::reset) deliberately adds tokens and is outside that bound.
///
/// # Limits
///
/// Capacity is exact for every `u32`. Time is tracked in nanoseconds since
/// construction and never wraps; the clock range is ~438 years, after which the
/// bucket stops refilling (it never over-grants). A bucket whose time to fill
/// from empty exceeds ~36.5 years tracks time in coarser ticks, which only
/// matters at that scale. `retry_after` is rounded up to the millisecond. A
/// plain token bucket has no queue, so a large request can be starved by a
/// stream of small ones.
///
/// A `Bucket<SystemClock>` is exactly one 64-byte cache line, so independent
/// buckets never falsely share.
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
    /// The accounting engine: the packed state, the rate, and the config.
    core: Core,
    /// The monotonic anchor elapsed time is measured from.
    created_at: Monotonic,
    /// The injected time source.
    clock: C,
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
        Self::with_config_and_clock(config, SystemClock::new())
    }
}

impl<C: Clock> Bucket<C> {
    /// Creates a bucket from a [`BucketConfig`] and a clock in one step,
    /// holding its initial tokens as of the clock's current reading.
    ///
    /// This is the clock-injection constructor: it reads the clock once and
    /// builds once, where `from_config(config).with_clock(clock)` builds a
    /// `SystemClock` bucket first and then rebuilds it. Use it wherever a
    /// consumer creates buckets with an injected clock (per-key buckets in a
    /// rate limiter, tests).
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{Bucket, BucketConfig};
    /// use clock_lib::ManualClock;
    /// use std::sync::Arc;
    /// use std::time::Duration;
    ///
    /// let clock = Arc::new(ManualClock::new());
    /// let config = BucketConfig::new(100, 10, Duration::from_secs(1), 0)?;
    /// let bucket = Bucket::with_config_and_clock(config, Arc::clone(&clock));
    ///
    /// assert_eq!(bucket.available(), 0);
    /// clock.advance(Duration::from_secs(1));
    /// assert_eq!(bucket.available(), 10);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[must_use]
    pub fn with_config_and_clock(config: BucketConfig, clock: C) -> Self {
        let created_at = clock.now();
        Self {
            core: Core::new(config, Duration::ZERO),
            created_at,
            clock,
        }
    }

    /// Replaces the bucket's time source, resetting it to its initial fill
    /// anchored at the new clock's current reading.
    ///
    /// Tokens already taken from `self` are forgotten: the result is a fresh
    /// bucket with the same configuration. Prefer
    /// [`with_config_and_clock`](Self::with_config_and_clock) when building
    /// from scratch; this remains for chaining onto the Tier-1 constructors,
    /// chiefly in tests, where injecting a
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
        Bucket::with_config_and_clock(self.core.config(), clock)
    }

    /// Attempts to take `n` tokens, returning the full [`Decision`].
    ///
    /// Brings the bucket current (lazy refill) and, if at least `n` tokens are
    /// available, deducts them and returns [`Decision::Allowed`]. Otherwise the
    /// bucket is left untouched and [`Decision::Denied`] carries the wait,
    /// rounded up to the millisecond, until the request would succeed.
    /// Requesting `0` always succeeds; requesting more than the capacity can
    /// never succeed (the denial's `retry_after` is [`Duration::MAX`]).
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
    #[inline]
    pub fn acquire(&self, n: u32) -> Decision {
        self.core.acquire(n, self.elapsed())
    }

    /// [`acquire`](Self::acquire) at a caller-supplied clock reading.
    ///
    /// `now` should come from this bucket's clock (or one that agrees with it).
    /// A reading earlier than the bucket's construction counts as the
    /// construction instant; a reading earlier than the last grant simply sees
    /// fewer tokens. Use it to read the clock once and decide for several
    /// buckets, or to make a decision at a known instant.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{Bucket, Decision};
    /// use clock_lib::{Clock, ManualClock};
    /// use std::sync::Arc;
    ///
    /// let clock = Arc::new(ManualClock::new());
    /// let a = Bucket::per_second(1).with_clock(Arc::clone(&clock));
    /// let b = Bucket::per_second(1).with_clock(Arc::clone(&clock));
    ///
    /// let now = clock.now(); // one read, two decisions
    /// assert_eq!(a.acquire_at(1, now), Decision::Allowed);
    /// assert_eq!(b.acquire_at(1, now), Decision::Allowed);
    /// ```
    #[inline]
    pub fn acquire_at(&self, n: u32, now: Monotonic) -> Decision {
        self.core.acquire(n, self.elapsed_at(now))
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
    #[inline]
    #[must_use]
    pub fn try_acquire(&self, n: u32) -> bool {
        self.acquire(n).is_allowed()
    }

    /// [`try_acquire`](Self::try_acquire) at a caller-supplied clock reading;
    /// see [`acquire_at`](Self::acquire_at).
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    /// use clock_lib::{Clock, SystemClock};
    ///
    /// let bucket = Bucket::per_second(2);
    /// let now = SystemClock::new().now();
    /// assert!(bucket.try_acquire_at(2, now));
    /// assert!(!bucket.try_acquire_at(1, now));
    /// ```
    #[inline]
    #[must_use]
    pub fn try_acquire_at(&self, n: u32, now: Monotonic) -> bool {
        self.acquire_at(n, now).is_allowed()
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
    #[inline]
    #[must_use]
    pub fn available(&self) -> u32 {
        self.core.available(self.elapsed())
    }

    /// [`available`](Self::available) at a caller-supplied clock reading; see
    /// [`acquire_at`](Self::acquire_at).
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    /// use clock_lib::{Clock, ManualClock};
    /// use std::sync::Arc;
    /// use std::time::Duration;
    ///
    /// let clock = Arc::new(ManualClock::new());
    /// let bucket = Bucket::per_second(10).with_clock(Arc::clone(&clock));
    /// assert!(bucket.try_acquire(10));
    /// clock.advance(Duration::from_millis(300));
    /// assert_eq!(bucket.available_at(clock.now()), 3);
    /// ```
    #[inline]
    #[must_use]
    pub fn available_at(&self, now: Monotonic) -> u32 {
        self.core.available(self.elapsed_at(now))
    }

    /// Returns the bucket's capacity (its burst ceiling), in whole tokens.
    ///
    /// Always equal to `self.config().capacity()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    ///
    /// assert_eq!(Bucket::per_second(64).capacity(), 64);
    /// assert_eq!(Bucket::per_second(10_000_000).capacity(), 10_000_000);
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
    /// use better_bucket::Bucket;
    /// use std::time::Duration;
    ///
    /// let bucket = Bucket::per_second(10);
    /// assert_eq!(bucket.config().refill_period(), Duration::from_secs(1));
    /// ```
    #[must_use]
    pub const fn config(&self) -> BucketConfig {
        self.core.config()
    }

    /// Returns the bucket's clock.
    ///
    /// Handy for reading the time once and passing it to the `*_at` methods of
    /// this and other buckets that share the clock.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    /// use clock_lib::Clock;
    ///
    /// let bucket = Bucket::per_second(5);
    /// let now = bucket.clock().now();
    /// assert!(bucket.try_acquire_at(1, now));
    /// ```
    #[must_use]
    pub const fn clock(&self) -> &C {
        &self.clock
    }

    /// Refills the bucket to full and marks it current as of now.
    ///
    /// Use it to discard accumulated debt and grant a fresh burst — for example at the start of a new billing
    /// window, or to clear a backlog after a dependency recovers. Long uptime
    /// needs no special handling: the clock never wraps.
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
        self.core.reset(self.elapsed());
    }

    /// [`reset`](Self::reset) at a caller-supplied clock reading; see
    /// [`acquire_at`](Self::acquire_at).
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    /// use clock_lib::Clock;
    ///
    /// let bucket = Bucket::per_second(4);
    /// assert!(bucket.try_acquire(4));
    /// let now = bucket.clock().now();
    /// bucket.reset_at(now);
    /// assert_eq!(bucket.available_at(now), 4);
    /// ```
    pub fn reset_at(&self, now: Monotonic) {
        self.core.reset(self.elapsed_at(now));
    }

    /// Elapsed time since construction, from the bucket's own clock.
    #[inline]
    fn elapsed(&self) -> Duration {
        self.elapsed_at(self.clock.now())
    }

    /// Elapsed time since construction at a supplied reading. A reading from
    /// before construction saturates to zero.
    #[inline]
    fn elapsed_at(&self, now: Monotonic) -> Duration {
        now.saturating_duration_since(self.created_at)
    }
}

/// Shows the capacity, the configuration, and the tokens available at the time
/// of formatting (which reads the clock).
impl<C: Clock> core::fmt::Debug for Bucket<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Bucket")
            .field("capacity", &self.capacity())
            .field("available", &self.available())
            .field("config", &self.config())
            .finish()
    }
}

/// The token-bucket surface a consumer depends on.
///
/// `TokenBucket` is the abstraction `rate-net` (and any other consumer) codes
/// against, so it can hold a bucket without naming its concrete clock type. It
/// mirrors the inherent methods of [`Bucket`]; see those for the detailed
/// contract of each. The trait is object-safe.
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
    #[inline]
    fn acquire(&self, n: u32) -> Decision {
        Self::acquire(self, n)
    }

    #[inline]
    fn try_acquire(&self, n: u32) -> bool {
        Self::try_acquire(self, n)
    }

    #[inline]
    fn available(&self) -> u32 {
        Self::available(self)
    }

    #[inline]
    fn capacity(&self) -> u32 {
        Self::capacity(self)
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{Bucket, TokenBucket};
    use crate::config::BucketConfig;
    use crate::decision::Decision;
    use clock_lib::{Clock, ManualClock, SystemClock};
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
    fn test_refill_after_long_idle_saturates_without_overflow() {
        // A multi-year idle gap fills to capacity — for any length, not only
        // when the gap happens to land well modulo 2^32 ms.
        let (clock, bucket) = manual_bucket(1_000);
        assert!(bucket.try_acquire(1_000));
        clock.advance(Duration::from_secs(60 * 60 * 24 * 365 * 5));
        assert_eq!(bucket.available(), 1_000);
        // And the bucket is still usable afterwards.
        assert!(bucket.try_acquire(1_000));
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
    fn test_high_contention_conserves_every_token() {
        // The accuracy certification: 16 threads hammer one no-refill bucket,
        // each taking 3 tokens at a time. Capacity is not a multiple of the take
        // size, so the last partial request cannot fit. With no refill, every
        // token must be accounted for — either granted or still available — with
        // none lost, duplicated, or handed out beyond capacity.
        const CAPACITY: u32 = 6_001;
        const THREADS: u32 = 16;
        const TAKE: u32 = 3;

        let clock = Arc::new(ManualClock::new()); // never advanced => no refill
        let bucket = Arc::new(Bucket::per_second(CAPACITY).with_clock(clock));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let bucket = Arc::clone(&bucket);
                thread::spawn(move || {
                    let mut taken = 0u32;
                    for _ in 0..CAPACITY {
                        if bucket.try_acquire(TAKE) {
                            taken += TAKE;
                        }
                    }
                    taken
                })
            })
            .collect();

        let granted: u32 = handles.into_iter().map(|h| h.join().unwrap()).sum();

        // No over-grant: never more than capacity (and a multiple of the take).
        assert!(granted <= CAPACITY, "over-grant: {granted} > {CAPACITY}");
        assert_eq!(granted % TAKE, 0, "a partial take was granted");
        // Exact conservation: granted + still-available == the initial capacity.
        // Any lost, duplicated, or corrupted token breaks this equality.
        assert_eq!(
            bucket.available(),
            CAPACITY - granted,
            "tokens were lost or corrupted under contention"
        );
    }

    #[test]
    fn test_system_clock_bucket_is_one_cache_line() {
        assert_eq!(core::mem::align_of::<Bucket<SystemClock>>(), 64);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(core::mem::size_of::<Bucket<SystemClock>>(), 64);
    }

    #[test]
    fn test_with_config_and_clock_builds_once_at_the_clock_reading() {
        let clock = Arc::new(ManualClock::new());
        clock.advance(Duration::from_secs(3_600)); // the clock is not at zero
        let config = BucketConfig::new(10, 10, Duration::from_secs(1), 0).unwrap();
        let bucket = Bucket::with_config_and_clock(config, Arc::clone(&clock));
        assert_eq!(bucket.available(), 0);
        assert_eq!(bucket.config(), config);
        clock.advance(Duration::from_millis(700));
        assert_eq!(bucket.available(), 7);
    }

    #[test]
    fn test_with_clock_on_a_used_bucket_starts_fresh() {
        let (_clock, bucket) = manual_bucket(10);
        assert!(bucket.try_acquire(10));
        let rebuilt = bucket.with_clock(Arc::new(ManualClock::new()));
        assert_eq!(rebuilt.available(), 10);
    }

    #[test]
    fn test_reading_before_construction_is_clamped() {
        let clock = Arc::new(ManualClock::new());
        let before = clock.now();
        clock.advance(Duration::from_secs(1));
        let config = BucketConfig::new(4, 4, Duration::from_secs(1), 1).unwrap();
        let bucket = Bucket::with_config_and_clock(config, Arc::clone(&clock));
        assert_eq!(bucket.available_at(before), 1);
    }

    #[test]
    fn test_clock_accessor_returns_the_injected_clock() {
        let (clock, bucket) = manual_bucket(1);
        clock.advance(Duration::from_secs(9));
        assert_eq!(bucket.clock().offset(), Duration::from_secs(9));
    }

    #[test]
    fn test_debug_shows_capacity_available_and_config() {
        let (_clock, bucket) = manual_bucket(3);
        assert!(bucket.try_acquire(1));
        let rendered = std::format!("{bucket:?}");
        assert!(rendered.starts_with("Bucket"));
        assert!(rendered.contains("capacity: 3"));
        assert!(rendered.contains("available: 2"));
        assert!(rendered.contains("config: BucketConfig"));
    }
}
