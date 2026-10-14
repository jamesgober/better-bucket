//! Tier-2 builder for explicit bucket configuration.

use core::time::Duration;

#[cfg(feature = "clock")]
use clock_lib::{Clock, SystemClock};

#[cfg(feature = "clock")]
use crate::bucket::Bucket;
use crate::config::BucketConfig;
use crate::error::BucketError;

/// A fluent builder for a [`Bucket`] when the Tier-1 constructors are not enough.
///
/// Set the capacity (the burst ceiling), the refill rate, and optionally the
/// initial fill, then call [`build`](Self::build). Anything left unset keeps its
/// default, and `build` validates the result through [`BucketConfig::new`], so
/// an unworkable combination is rejected rather than producing a misbehaving
/// bucket.
///
/// Capacity and burst are the same thing for a token bucket: the bucket holds at
/// most `capacity` tokens, so the largest single acquire it can ever grant — the
/// burst — is `capacity`.
///
/// [`build`](Self::build) produces a [`SystemClock`](clock_lib::SystemClock)
/// bucket; [`build_with_clock`](Self::build_with_clock) takes any clock, and
/// [`build_config`](Self::build_config) stops at the validated
/// [`BucketConfig`] (the `no_std` path, for a [`BucketCore`](crate::BucketCore)).
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "clock")] {
/// use better_bucket::Bucket;
/// use std::time::Duration;
///
/// // Burst up to 1000, refill 50/second, start empty.
/// let bucket = Bucket::builder()
///     .capacity(1000)
///     .refill(50, Duration::from_secs(1))
///     .initial(0)
///     .build()?;
///
/// assert_eq!(bucket.capacity(), 1000);
/// assert_eq!(bucket.available(), 0);
/// # }
/// # Ok::<(), better_bucket::BucketError>(())
/// ```
#[derive(Debug, Clone, Default)]
#[must_use = "a builder does nothing until `.build()` is called"]
pub struct BucketBuilder {
    capacity: u32,
    refill_amount: u32,
    refill_period: Duration,
    initial: Option<u32>,
}

impl BucketBuilder {
    /// Starts a builder with every field at its default (which `build` rejects
    /// until at least a capacity and refill rate are set).
    pub const fn new() -> Self {
        Self {
            capacity: 0,
            refill_amount: 0,
            refill_period: Duration::ZERO,
            initial: None,
        }
    }

    /// Sets the capacity — the maximum tokens the bucket holds, and therefore
    /// the largest burst it can grant at once. Required.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::BucketBuilder;
    /// use std::time::Duration;
    ///
    /// let config = BucketBuilder::new()
    ///     .capacity(200)
    ///     .refill(200, Duration::from_secs(1))
    ///     .build_config()?;
    /// assert_eq!(config.capacity(), 200);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    pub const fn capacity(mut self, capacity: u32) -> Self {
        self.capacity = capacity;
        self
    }

    /// Sets the sustained refill rate: `amount` tokens every `period`. Required.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::BucketBuilder;
    /// use std::time::Duration;
    ///
    /// // 10 tokens every 250ms.
    /// let config = BucketBuilder::new()
    ///     .capacity(10)
    ///     .refill(10, Duration::from_millis(250))
    ///     .build_config()?;
    /// assert_eq!(config.refill_period(), Duration::from_millis(250));
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    pub const fn refill(mut self, amount: u32, period: Duration) -> Self {
        self.refill_amount = amount;
        self.refill_period = period;
        self
    }

    /// Sets the initial number of tokens. Defaults to the capacity (the bucket
    /// starts full); values above the capacity are clamped to it.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::BucketBuilder;
    /// use std::time::Duration;
    ///
    /// let config = BucketBuilder::new()
    ///     .capacity(100)
    ///     .refill(100, Duration::from_secs(1))
    ///     .initial(0) // start empty instead of full
    ///     .build_config()?;
    /// assert_eq!(config.initial(), 0);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    pub const fn initial(mut self, initial: u32) -> Self {
        self.initial = Some(initial);
        self
    }

    /// Validates the configuration and returns it, without building a bucket.
    ///
    /// Available in every build, including `no_std`: pair it with
    /// [`BucketCore::new`](crate::BucketCore::new), or keep the config to build
    /// several buckets from.
    ///
    /// # Errors
    ///
    /// The same as [`BucketConfig::new`]: zero capacity, zero refill amount, or
    /// zero refill period.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{BucketBuilder, BucketCore};
    /// use std::time::Duration;
    ///
    /// let config = BucketBuilder::new()
    ///     .capacity(20)
    ///     .refill(5, Duration::from_secs(1))
    ///     .build_config()?;
    /// let bucket = BucketCore::new(config, Duration::ZERO);
    /// assert_eq!(bucket.available_at(Duration::ZERO), 20);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    pub const fn build_config(self) -> Result<BucketConfig, BucketError> {
        let initial = match self.initial {
            Some(initial) => initial,
            None => self.capacity,
        };
        BucketConfig::new(
            self.capacity,
            self.refill_amount,
            self.refill_period,
            initial,
        )
    }

    /// Validates the configuration and builds the bucket.
    ///
    /// # Errors
    ///
    /// Returns a [`BucketError`] for the same reasons as
    /// [`BucketConfig::new`]: zero capacity, zero refill amount, or zero refill
    /// period. A freshly created builder fails this way until a capacity and
    /// refill rate are set.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::{Bucket, BucketError};
    ///
    /// // Nothing configured yet → rejected.
    /// let err = Bucket::builder().build().unwrap_err();
    /// assert_eq!(err, BucketError::ZeroCapacity);
    /// ```
    #[cfg(feature = "clock")]
    #[cfg_attr(docsrs, doc(cfg(feature = "clock")))]
    pub fn build(self) -> Result<Bucket<SystemClock>, BucketError> {
        self.build_with_clock(SystemClock::new())
    }

    /// Validates the configuration and builds a bucket driven by `clock`.
    ///
    /// One build and one clock read, the builder form of
    /// [`Bucket::with_config_and_clock`].
    ///
    /// # Errors
    ///
    /// The same as [`build`](Self::build).
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
    /// let bucket = Bucket::builder()
    ///     .capacity(10)
    ///     .refill(1, Duration::from_secs(1))
    ///     .initial(0)
    ///     .build_with_clock(Arc::clone(&clock))?;
    /// clock.advance(Duration::from_secs(3));
    /// assert_eq!(bucket.available(), 3);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[cfg(feature = "clock")]
    #[cfg_attr(docsrs, doc(cfg(feature = "clock")))]
    pub fn build_with_clock<C: Clock>(self, clock: C) -> Result<Bucket<C>, BucketError> {
        Ok(Bucket::with_config_and_clock(self.build_config()?, clock))
    }
}

#[cfg(feature = "clock")]
impl Bucket<SystemClock> {
    /// Starts a [`BucketBuilder`] for explicit configuration.
    ///
    /// The Tier-2 entry point, for when [`per_second`](Self::per_second) /
    /// [`per_duration`](Self::per_duration) are not enough — e.g. a capacity
    /// and refill rate that differ, or a non-full initial fill.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    /// use std::time::Duration;
    ///
    /// let bucket = Bucket::builder()
    ///     .capacity(500)
    ///     .refill(100, Duration::from_secs(1))
    ///     .build()?;
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    #[must_use = "a builder does nothing until `.build()` is called"]
    pub const fn builder() -> BucketBuilder {
        BucketBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::BucketBuilder;
    #[cfg(feature = "clock")]
    use crate::bucket::Bucket;
    use crate::error::BucketError;
    use core::time::Duration;

    #[test]
    fn test_build_config_applies_every_field() {
        let config = BucketBuilder::new()
            .capacity(500)
            .refill(100, Duration::from_secs(2))
            .initial(7)
            .build_config()
            .unwrap();
        assert_eq!(config.capacity(), 500);
        assert_eq!(config.refill_amount(), 100);
        assert_eq!(config.refill_period(), Duration::from_secs(2));
        assert_eq!(config.initial(), 7);
    }

    #[test]
    fn test_build_config_defaults_initial_to_capacity() {
        let config = BucketBuilder::default()
            .capacity(9)
            .refill(1, Duration::from_secs(1))
            .build_config()
            .unwrap();
        assert_eq!(config.initial(), 9);
    }

    #[test]
    fn test_builder_is_usable_in_const_context() {
        const BUILDER: BucketBuilder = BucketBuilder::new()
            .capacity(3)
            .refill(3, Duration::from_secs(1))
            .initial(1);
        assert_eq!(BUILDER.build_config().unwrap().initial(), 1);
    }

    #[test]
    fn test_build_config_rejects_like_new() {
        assert_eq!(
            BucketBuilder::new().build_config().unwrap_err(),
            BucketError::ZeroCapacity
        );
        assert_eq!(
            BucketBuilder::new().capacity(1).build_config().unwrap_err(),
            BucketError::ZeroRefillAmount
        );
        assert_eq!(
            BucketBuilder::new()
                .capacity(1)
                .refill(1, Duration::ZERO)
                .build_config()
                .unwrap_err(),
            BucketError::ZeroRefillPeriod
        );
    }

    #[cfg(feature = "clock")]
    #[test]
    fn test_builds_configured_bucket() {
        let bucket = Bucket::builder()
            .capacity(500)
            .refill(100, Duration::from_secs(1))
            .initial(0)
            .build()
            .unwrap();
        assert_eq!(bucket.capacity(), 500);
        assert_eq!(bucket.available(), 0);
        assert_eq!(bucket.config().refill_amount(), 100);
    }

    #[cfg(feature = "clock")]
    #[test]
    fn test_initial_defaults_to_full() {
        let bucket = Bucket::builder()
            .capacity(40)
            .refill(40, Duration::from_secs(1))
            .build()
            .unwrap();
        assert_eq!(bucket.available(), 40);
    }

    #[cfg(feature = "clock")]
    #[test]
    fn test_empty_builder_is_rejected() {
        assert_eq!(
            BucketBuilder::new().build().unwrap_err(),
            BucketError::ZeroCapacity
        );
    }

    #[cfg(feature = "clock")]
    #[test]
    fn test_missing_refill_is_rejected() {
        let err = Bucket::builder().capacity(10).build().unwrap_err();
        assert_eq!(err, BucketError::ZeroRefillAmount);
    }

    #[cfg(feature = "clock")]
    #[test]
    fn test_zero_period_is_rejected() {
        let err = Bucket::builder()
            .capacity(10)
            .refill(10, Duration::ZERO)
            .build()
            .unwrap_err();
        assert_eq!(err, BucketError::ZeroRefillPeriod);
    }

    #[cfg(feature = "clock")]
    #[test]
    fn test_build_with_clock_uses_the_injected_clock() {
        use clock_lib::ManualClock;
        use std::sync::Arc;

        let clock = Arc::new(ManualClock::new());
        let bucket = Bucket::builder()
            .capacity(4)
            .refill(4, Duration::from_secs(1))
            .initial(0)
            .build_with_clock(Arc::clone(&clock))
            .unwrap();
        assert_eq!(bucket.available(), 0);
        clock.advance(Duration::from_millis(500));
        assert_eq!(bucket.available(), 2);
        assert!(Bucket::builder().build_with_clock(clock).is_err());
    }
}
