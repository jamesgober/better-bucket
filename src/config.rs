//! Validated bucket configuration.

use core::time::Duration;

use crate::error::BucketError;

/// The parameters that define a token bucket.
///
/// A bucket holds up to `capacity` tokens (the burst ceiling) and accrues
/// `refill_amount` tokens every `refill_period` (the sustained rate). It starts
/// with `initial` tokens. These four numbers fully describe the bucket's
/// behaviour; everything else is accounting.
///
/// Construct one with [`BucketConfig::new`], which rejects values that cannot
/// describe a working bucket (see [`BucketError`]). The Tier-1 constructors
/// [`Bucket::per_second`](crate::Bucket::per_second) and
/// [`Bucket::per_duration`](crate::Bucket::per_duration) build a config for you
/// for the common case.
///
/// `initial` is clamped to `capacity`: asking a bucket to start with more
/// tokens than it can hold simply starts it full.
///
/// Every value `new` accepts is represented exactly by the bucket: there is no
/// capacity ceiling below `u32::MAX` and no rate too slow to refill, so
/// [`Bucket::capacity`](crate::Bucket::capacity) always equals
/// [`capacity`](Self::capacity).
///
/// # Examples
///
/// ```
/// use better_bucket::BucketConfig;
/// use std::time::Duration;
///
/// // 500-token burst ceiling, refilling 100 tokens/second, starting empty.
/// let config = BucketConfig::new(500, 100, Duration::from_secs(1), 0)?;
/// assert_eq!(config.capacity(), 500);
/// assert_eq!(config.initial(), 0);
/// # Ok::<(), better_bucket::BucketError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BucketConfig {
    capacity: u32,
    refill_amount: u32,
    refill_period: Duration,
    initial: u32,
}

impl BucketConfig {
    /// Builds a validated configuration.
    ///
    /// # Parameters
    ///
    /// - `capacity` — the maximum tokens the bucket holds (its burst size).
    ///   Must be greater than zero.
    /// - `refill_amount` — tokens added every `refill_period`. Must be greater
    ///   than zero.
    /// - `refill_period` — the period over which `refill_amount` accrues. Must
    ///   be non-zero.
    /// - `initial` — tokens present at construction, clamped to `capacity`.
    ///
    /// # Errors
    ///
    /// - [`BucketError::ZeroCapacity`] if `capacity` is `0`.
    /// - [`BucketError::ZeroRefillAmount`] if `refill_amount` is `0`.
    /// - [`BucketError::ZeroRefillPeriod`] if `refill_period` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::BucketConfig;
    /// use std::time::Duration;
    ///
    /// let config = BucketConfig::new(100, 100, Duration::from_secs(1), 100)?;
    /// assert_eq!(config.capacity(), 100);
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    ///
    /// `initial` larger than `capacity` is clamped rather than rejected:
    ///
    /// ```
    /// use better_bucket::BucketConfig;
    /// use std::time::Duration;
    ///
    /// let config = BucketConfig::new(100, 100, Duration::from_secs(1), 999)?;
    /// assert_eq!(config.initial(), 100); // clamped to capacity
    /// # Ok::<(), better_bucket::BucketError>(())
    /// ```
    ///
    /// `new` is a `const fn`, so a configuration can be validated at compile
    /// time — handy for a `static` bucket on an embedded target:
    ///
    /// ```
    /// use better_bucket::BucketConfig;
    /// use std::time::Duration;
    ///
    /// const LIMIT: BucketConfig = match BucketConfig::new(10, 10, Duration::from_secs(1), 10) {
    ///     Ok(config) => config,
    ///     Err(_) => panic!("invalid bucket configuration"),
    /// };
    /// assert_eq!(LIMIT.capacity(), 10);
    /// ```
    pub const fn new(
        capacity: u32,
        refill_amount: u32,
        refill_period: Duration,
        initial: u32,
    ) -> Result<Self, BucketError> {
        if capacity == 0 {
            return Err(BucketError::ZeroCapacity);
        }
        if refill_amount == 0 {
            return Err(BucketError::ZeroRefillAmount);
        }
        if refill_period.is_zero() {
            return Err(BucketError::ZeroRefillPeriod);
        }
        Ok(Self::raw(capacity, refill_amount, refill_period, initial))
    }

    /// The maximum number of tokens the bucket holds (its burst ceiling).
    #[must_use]
    pub const fn capacity(&self) -> u32 {
        self.capacity
    }

    /// The number of tokens added each [`refill_period`](Self::refill_period).
    #[must_use]
    pub const fn refill_amount(&self) -> u32 {
        self.refill_amount
    }

    /// The period over which [`refill_amount`](Self::refill_amount) accrues.
    #[must_use]
    pub const fn refill_period(&self) -> Duration {
        self.refill_period
    }

    /// The number of tokens the bucket starts with.
    #[must_use]
    pub const fn initial(&self) -> u32 {
        self.initial
    }

    /// Builds a configuration without validation, used by the infallible Tier-1
    /// constructors. A degenerate request (zero capacity or zero refill) yields
    /// a bucket that grants nothing, which is well-defined and safe rather than
    /// a panic.
    pub(crate) const fn raw(
        capacity: u32,
        refill_amount: u32,
        refill_period: Duration,
        initial: u32,
    ) -> Self {
        let initial = if initial < capacity {
            initial
        } else {
            capacity
        };
        Self {
            capacity,
            refill_amount,
            refill_period,
            initial,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::BucketConfig;
    use crate::error::BucketError;
    use core::time::Duration;

    #[test]
    fn test_new_accepts_valid_parameters() {
        let config = BucketConfig::new(100, 50, Duration::from_secs(1), 25).unwrap();
        assert_eq!(config.capacity(), 100);
        assert_eq!(config.refill_amount(), 50);
        assert_eq!(config.refill_period(), Duration::from_secs(1));
        assert_eq!(config.initial(), 25);
    }

    #[test]
    fn test_new_rejects_zero_capacity() {
        let err = BucketConfig::new(0, 10, Duration::from_secs(1), 0).unwrap_err();
        assert_eq!(err, BucketError::ZeroCapacity);
    }

    #[test]
    fn test_new_rejects_zero_refill_amount() {
        let err = BucketConfig::new(10, 0, Duration::from_secs(1), 0).unwrap_err();
        assert_eq!(err, BucketError::ZeroRefillAmount);
    }

    #[test]
    fn test_new_rejects_zero_refill_period() {
        let err = BucketConfig::new(10, 10, Duration::ZERO, 0).unwrap_err();
        assert_eq!(err, BucketError::ZeroRefillPeriod);
    }

    #[test]
    fn test_new_clamps_initial_to_capacity() {
        let config = BucketConfig::new(10, 10, Duration::from_secs(1), 999).unwrap();
        assert_eq!(config.initial(), 10);
    }
}
