//! Tier-2 builder for explicit bucket configuration.

use core::time::Duration;

use clock_lib::SystemClock;

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
/// For a custom time source, chain [`Bucket::with_clock`] onto the built bucket;
/// the builder itself always produces a [`SystemClock`](clock_lib::SystemClock)
/// bucket.
///
/// # Examples
///
/// ```
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
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the capacity — the maximum tokens the bucket holds, and therefore
    /// the largest burst it can grant at once. Required.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    /// use std::time::Duration;
    ///
    /// let bucket = Bucket::builder()
    ///     .capacity(200)
    ///     .refill(200, Duration::from_secs(1))
    ///     .build()?;
    /// assert_eq!(bucket.capacity(), 200);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    pub fn capacity(mut self, capacity: u32) -> Self {
        self.capacity = capacity;
        self
    }

    /// Sets the sustained refill rate: `amount` tokens every `period`. Required.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Bucket;
    /// use std::time::Duration;
    ///
    /// // 10 tokens every 250ms.
    /// let bucket = Bucket::builder()
    ///     .capacity(10)
    ///     .refill(10, Duration::from_millis(250))
    ///     .build()?;
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    pub fn refill(mut self, amount: u32, period: Duration) -> Self {
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
    /// use better_bucket::Bucket;
    /// use std::time::Duration;
    ///
    /// let bucket = Bucket::builder()
    ///     .capacity(100)
    ///     .refill(100, Duration::from_secs(1))
    ///     .initial(0) // start empty instead of full
    ///     .build()?;
    /// assert_eq!(bucket.available(), 0);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    pub fn initial(mut self, initial: u32) -> Self {
        self.initial = Some(initial);
        self
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
    pub fn build(self) -> Result<Bucket<SystemClock>, BucketError> {
        let initial = self.initial.unwrap_or(self.capacity);
        let config = BucketConfig::new(
            self.capacity,
            self.refill_amount,
            self.refill_period,
            initial,
        )?;
        Ok(Bucket::from_config(config))
    }
}

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
    pub fn builder() -> BucketBuilder {
        BucketBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::BucketBuilder;
    use crate::bucket::Bucket;
    use crate::error::BucketError;
    use core::time::Duration;

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

    #[test]
    fn test_initial_defaults_to_full() {
        let bucket = Bucket::builder()
            .capacity(40)
            .refill(40, Duration::from_secs(1))
            .build()
            .unwrap();
        assert_eq!(bucket.available(), 40);
    }

    #[test]
    fn test_empty_builder_is_rejected() {
        assert_eq!(
            BucketBuilder::new().build().unwrap_err(),
            BucketError::ZeroCapacity
        );
    }

    #[test]
    fn test_missing_refill_is_rejected() {
        let err = Bucket::builder().capacity(10).build().unwrap_err();
        assert_eq!(err, BucketError::ZeroRefillAmount);
    }

    #[test]
    fn test_zero_period_is_rejected() {
        let err = Bucket::builder()
            .capacity(10)
            .refill(10, Duration::ZERO)
            .build()
            .unwrap_err();
        assert_eq!(err, BucketError::ZeroRefillPeriod);
    }
}
