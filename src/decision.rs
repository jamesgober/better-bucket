//! The outcome of an acquire attempt.

use core::time::Duration;

/// The result of attempting to take tokens from a bucket.
///
/// Returned by [`Bucket::acquire`](crate::Bucket::acquire). The acquire path is
/// infallible — there is no error case, only an allow/deny outcome — so this is
/// a plain enum rather than a `Result`. When the request is denied, the
/// decision carries how long the caller should wait before enough tokens will
/// have accrued, which is exactly what a downstream limiter (e.g. `rate-net`)
/// needs to populate a `Retry-After`.
///
/// `#[non_exhaustive]` so future variants can be added without breaking callers;
/// match with a wildcard arm, or use the [`is_allowed`](Self::is_allowed) /
/// [`retry_after`](Self::retry_after) helpers.
///
/// # Examples
///
/// ```
/// use better_bucket::{Bucket, Decision};
///
/// let bucket = Bucket::per_second(1);
/// match bucket.acquire(1) {
///     Decision::Allowed => { /* serve the request */ }
///     Decision::Denied { retry_after } => {
///         // tell the caller when to come back
///         let _ = retry_after;
///     }
///     _ => {}
/// }
/// ```
#[must_use]
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The tokens were granted and have been deducted from the bucket.
    Allowed,
    /// The request was refused because the bucket did not hold enough tokens.
    Denied {
        /// The minimum wait until the bucket will hold enough tokens to grant
        /// the same request. [`Duration::MAX`] means the request can never
        /// succeed (it asked for more tokens than the bucket's capacity).
        retry_after: Duration,
    },
}

impl Decision {
    /// Returns `true` if the request was granted.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Decision;
    ///
    /// assert!(Decision::Allowed.is_allowed());
    /// ```
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// Returns `true` if the request was refused.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Decision;
    /// use std::time::Duration;
    ///
    /// let denied = Decision::Denied { retry_after: Duration::from_millis(250) };
    /// assert!(denied.is_denied());
    /// ```
    #[must_use]
    pub const fn is_denied(&self) -> bool {
        !self.is_allowed()
    }

    /// Returns the wait until the request would succeed, or `None` if it was
    /// allowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use better_bucket::Decision;
    /// use std::time::Duration;
    ///
    /// let denied = Decision::Denied { retry_after: Duration::from_millis(250) };
    /// assert_eq!(denied.retry_after(), Some(Duration::from_millis(250)));
    /// assert_eq!(Decision::Allowed.retry_after(), None);
    /// ```
    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Denied { retry_after } => Some(*retry_after),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Decision;
    use core::time::Duration;

    #[test]
    fn test_allowed_predicates() {
        let allowed = Decision::Allowed;
        assert!(allowed.is_allowed());
        assert!(!allowed.is_denied());
        assert_eq!(allowed.retry_after(), None);
    }

    #[test]
    fn test_denied_predicates() {
        let denied = Decision::Denied {
            retry_after: Duration::from_secs(2),
        };
        assert!(denied.is_denied());
        assert!(!denied.is_allowed());
        assert_eq!(denied.retry_after(), Some(Duration::from_secs(2)));
    }
}
