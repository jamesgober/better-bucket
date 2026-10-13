//! Decision counters, behind the `metrics` feature.

/// A snapshot of a bucket's decision counters.
///
/// Returned by [`Bucket::metrics`](crate::Bucket::metrics) and
/// [`BucketCore::metrics`](crate::BucketCore::metrics) when the `metrics`
/// feature is on. The counters are cumulative since the bucket was built, are
/// updated with relaxed atomics on every decision, and wrap only after 2^64
/// events. With the feature off they do not exist at all: no field, no atomic
/// increment, no cost.
///
/// What the counters mean:
///
/// - `allowed` — calls that granted their request. A request for zero tokens
///   counts here, as does a successful reservation or a partial grant that took
///   at least one token.
/// - `denied` — calls that granted nothing for a non-zero request. A steadily
///   climbing deny rate means the configured quota is below demand; a sudden
///   spike usually means a burst or a misbehaving client.
/// - `tokens_granted` — total tokens handed out. Divided by elapsed time it is
///   the bucket's achieved throughput, which should never exceed the configured
///   rate by more than the capacity.
///
/// `reset` and `refund` are not counted: they move tokens, they are not
/// decisions.
///
/// # Examples
///
/// ```
/// use better_bucket::{BucketConfig, BucketCore};
/// use core::time::Duration;
///
/// let config = BucketConfig::new(2, 2, Duration::from_secs(1), 2)?;
/// let bucket = BucketCore::new(config, Duration::ZERO);
/// assert!(bucket.try_acquire_at(2, Duration::ZERO));
/// assert!(!bucket.try_acquire_at(1, Duration::ZERO));
///
/// let metrics = bucket.metrics();
/// assert_eq!(metrics.allowed, 1);
/// assert_eq!(metrics.denied, 1);
/// assert_eq!(metrics.tokens_granted, 2);
/// # Ok::<(), better_bucket::BucketError>(())
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BucketMetrics {
    /// Calls that granted their request.
    pub allowed: u64,
    /// Calls that granted nothing for a non-zero request.
    pub denied: u64,
    /// Total tokens granted.
    pub tokens_granted: u64,
}

impl BucketMetrics {
    pub(crate) const fn new(allowed: u64, denied: u64, tokens_granted: u64) -> Self {
        Self {
            allowed,
            denied,
            tokens_granted,
        }
    }
}
