//! Construction-time configuration errors.
//!
//! The acquire path is infallible — it returns a [`Decision`](crate::Decision),
//! never a `Result`. The only fallible operation in the crate is building a
//! [`BucketConfig`](crate::BucketConfig): a configuration that cannot describe a
//! working bucket is rejected up front rather than producing a bucket that
//! misbehaves later.
//!
//! [`BucketError`] implements [`error_forge::ForgeError`], so it slots into the
//! portfolio error stack (kinds, captions, the central error hook) the same way
//! every other domain error does.

use core::fmt;

use error_forge::ForgeError;

/// A configuration rejected at construction time.
///
/// Returned by [`BucketConfig::new`](crate::BucketConfig::new) when the supplied
/// values cannot describe a working token bucket. Each variant names exactly
/// which constraint was violated so the caller can correct the specific field.
///
/// The enum is `#[non_exhaustive]`: future releases may add validation rules
/// (and therefore variants) without it being a breaking change, so a `match`
/// on it must include a wildcard arm.
///
/// # Examples
///
/// ```
/// use better_bucket::{BucketConfig, BucketError};
/// use std::time::Duration;
///
/// let err = BucketConfig::new(0, 10, Duration::from_secs(1), 0).unwrap_err();
/// assert_eq!(err, BucketError::ZeroCapacity);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketError {
    /// The capacity was zero. A bucket that can hold no tokens can never grant
    /// one; supply a capacity of at least `1`.
    ZeroCapacity,
    /// The refill amount was zero. A bucket that accrues no tokens over time
    /// would only ever deplete; supply a refill amount of at least `1`.
    ZeroRefillAmount,
    /// The refill period was zero. Tokens accrue *per period*, so a zero-length
    /// period is undefined (it implies an infinite rate); supply a non-zero
    /// [`Duration`](core::time::Duration).
    ZeroRefillPeriod,
}

impl fmt::Display for BucketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ZeroCapacity => "bucket capacity must be greater than zero",
            Self::ZeroRefillAmount => "refill amount must be greater than zero",
            Self::ZeroRefillPeriod => "refill period must be greater than zero",
        };
        f.write_str(message)
    }
}

impl std::error::Error for BucketError {}

impl ForgeError for BucketError {
    fn kind(&self) -> &'static str {
        match self {
            Self::ZeroCapacity => "ZeroCapacity",
            Self::ZeroRefillAmount => "ZeroRefillAmount",
            Self::ZeroRefillPeriod => "ZeroRefillPeriod",
        }
    }

    fn caption(&self) -> &'static str {
        "Invalid bucket configuration"
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::BucketError;
    use error_forge::ForgeError;

    #[test]
    fn test_display_names_the_violated_constraint() {
        assert!(BucketError::ZeroCapacity.to_string().contains("capacity"));
        assert!(BucketError::ZeroRefillAmount.to_string().contains("amount"));
        assert!(BucketError::ZeroRefillPeriod.to_string().contains("period"));
    }

    #[test]
    fn test_forge_kind_matches_variant() {
        assert_eq!(BucketError::ZeroCapacity.kind(), "ZeroCapacity");
        assert_eq!(BucketError::ZeroRefillAmount.kind(), "ZeroRefillAmount");
        assert_eq!(BucketError::ZeroRefillPeriod.kind(), "ZeroRefillPeriod");
    }

    #[test]
    fn test_config_errors_are_not_retryable() {
        // A bad configuration will not fix itself on retry.
        assert!(!BucketError::ZeroCapacity.is_retryable());
    }
}
