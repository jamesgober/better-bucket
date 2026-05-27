//! The single token bucket and the `TokenBucket` trait.
//!
//! This is the `0.2` foundation implementation: correct, single-threaded-clean,
//! and behind a `Mutex` for interior mutability. It is deliberately *not* the
//! lock-free, allocation-free, cache-aligned core — that replaces the internals
//! wholesale in `0.3` without changing this public surface.

use core::time::Duration;
use std::sync::{Mutex, MutexGuard, PoisonError};

use clock_lib::{Clock, Monotonic, SystemClock};

use crate::config::BucketConfig;
use crate::decision::Decision;

/// Tokens are tracked in thousandths (millitokens) so refill can accrue at
/// sub-token resolution without losing fractions across calls.
const MILLI: u64 = 1_000;

/// The mutable accounting carried across calls, guarded by the bucket's mutex.
#[derive(Debug)]
struct State {
    /// Tokens currently available, in millitokens.
    millitokens: u64,
    /// The monotonic reading at which `millitokens` was last brought current.
    last_refill: Monotonic,
}

/// A token bucket: a counter that refills over time and grants tokens on demand.
///
/// A bucket holds up to its capacity in tokens, accrues more at a fixed rate,
/// and hands them out when asked. Refill is **lazy** — no background thread, no
/// timer: the token count is brought current from the monotonic clock at the
/// moment you call [`acquire`](Self::acquire), [`try_acquire`](Self::try_acquire),
/// or [`available`](Self::available).
///
/// The type parameter `C` is the time source. It defaults to
/// [`SystemClock`](clock_lib::SystemClock) (the OS monotonic clock); inject a
/// [`ManualClock`](clock_lib::ManualClock) with [`with_clock`](Self::with_clock)
/// to drive time by hand in tests. `Bucket` is `Send + Sync` whenever its clock
/// is, which every [`Clock`] implementation guarantees.
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
#[derive(Debug)]
pub struct Bucket<C: Clock = SystemClock> {
    config: BucketConfig,
    clock: C,
    state: Mutex<State>,
}

/// Constructs a bucket from a finished config and a clock, anchoring the refill
/// clock at the supplied clock's current reading and filling to `initial`.
fn build<C: Clock>(config: BucketConfig, clock: C) -> Bucket<C> {
    let last_refill = clock.now();
    Bucket {
        state: Mutex::new(State {
            millitokens: u64::from(config.initial()) * MILLI,
            last_refill,
        }),
        config,
        clock,
    }
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

/// Brings `state` current as of `now`, accruing lazy refill (saturating, never
/// exceeding capacity).
fn refill(state: &mut State, config: &BucketConfig, now: Monotonic) {
    let amount = config.refill_amount();
    let period_nanos = config.refill_period().as_nanos();
    if amount == 0 || period_nanos == 0 {
        // A bucket with no refill rate is static; nothing accrues.
        return;
    }
    let elapsed_nanos = now.saturating_duration_since(state.last_refill).as_nanos();
    if elapsed_nanos == 0 {
        return;
    }
    let rate_milli = u128::from(amount) * u128::from(MILLI);
    let accrued = elapsed_nanos.saturating_mul(rate_milli) / period_nanos;
    if accrued == 0 {
        // Not enough time for a whole millitoken yet — keep `last_refill` so
        // the elapsed time accumulates toward the next accrual.
        return;
    }
    let cap_milli = u128::from(config.capacity()) * u128::from(MILLI);
    let refilled = u128::from(state.millitokens)
        .saturating_add(accrued)
        .min(cap_milli);
    state.millitokens = u64::try_from(refilled).unwrap_or(u64::MAX);
    state.last_refill = now;
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
        self.available_inner()
    }

    /// Returns the bucket's capacity (its burst ceiling).
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
        self.config.capacity()
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

    fn lock(&self) -> MutexGuard<'_, State> {
        // The critical section never panics, so the mutex is never genuinely
        // poisoned; recover the guard regardless rather than propagate.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn acquire_inner(&self, n: u32) -> Decision {
        if n == 0 {
            // Zero tokens are always available, even from an empty bucket.
            return Decision::Allowed;
        }
        if u64::from(n) > u64::from(self.config.capacity()) {
            // More than the bucket can ever hold: it can never be granted.
            return Decision::Denied {
                retry_after: Duration::MAX,
            };
        }

        let now = self.clock.now();
        let need = u64::from(n) * MILLI;
        let mut state = self.lock();
        refill(&mut state, &self.config, now);

        if state.millitokens >= need {
            state.millitokens -= need;
            Decision::Allowed
        } else {
            let deficit = need - state.millitokens;
            drop(state);
            Decision::Denied {
                retry_after: self.time_for(deficit),
            }
        }
    }

    fn available_inner(&self) -> u32 {
        let now = self.clock.now();
        let mut state = self.lock();
        refill(&mut state, &self.config, now);
        u32::try_from(state.millitokens / MILLI).unwrap_or(u32::MAX)
    }

    /// The minimum time for `deficit_milli` millitokens to accrue at the
    /// configured rate, rounded up. [`Duration::MAX`] if the bucket never
    /// refills.
    fn time_for(&self, deficit_milli: u64) -> Duration {
        let amount = self.config.refill_amount();
        let period_nanos = self.config.refill_period().as_nanos();
        if amount == 0 || period_nanos == 0 {
            return Duration::MAX;
        }
        let rate_milli = u128::from(amount) * u128::from(MILLI);
        let numerator = u128::from(deficit_milli)
            .saturating_mul(period_nanos)
            .saturating_add(rate_milli - 1);
        let nanos = numerator / rate_milli;
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
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
        self.available_inner()
    }

    fn capacity(&self) -> u32 {
        self.config.capacity()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{Bucket, TokenBucket};
    use crate::decision::Decision;
    use clock_lib::{ManualClock, SystemClock};
    use core::time::Duration;
    use std::sync::Arc;

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
    fn test_zero_rate_is_deny_all() {
        let bucket = Bucket::per_second(0);
        assert_eq!(bucket.capacity(), 0);
        assert_eq!(bucket.available(), 0);
        assert!(!bucket.try_acquire(1));
        assert!(bucket.try_acquire(0));
    }

    #[test]
    fn test_trait_object_safe_surface() {
        // The trait is usable through a reference, which is how `rate-net`
        // holds a bucket without naming its clock type.
        let (_clock, bucket) = manual_bucket(4);
        let as_trait: &dyn TokenBucket = &bucket;
        assert_eq!(as_trait.capacity(), 4);
        assert!(as_trait.try_acquire(4));
        assert!(!as_trait.try_acquire(1));
    }
}
