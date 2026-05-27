//! # better-bucket
//!
//! A genuinely better token bucket for Rust. The hot path — `try_acquire` — is
//! designed to be **lock-free**, **allocation-free**, and **cache-aligned**: a
//! single compare-and-swap over a packed `(tokens, last_refill_tick)` word.
//! Refill is **lazy**, computed from a monotonic clock the instant you ask, so
//! an idle bucket costs nothing — no background timer thread, no per-tick
//! wakeups. The defining correctness property is that the bucket **never
//! over-grants**: across any concurrent interleaving, the total tokens handed
//! out never exceed capacity plus accrued refill.
//!
//! The crate is a single-purpose primitive. It owns token-bucket accounting and
//! nothing else, so it can sit at the bottom of a dependency tree (its first
//! consumer is the `rate-net` rate limiter) without dragging in an async
//! runtime or a keyed-state store.
//!
//! ## Status
//!
//! Pre-1.0, under active development. The `0.3` release ships the **lock-free
//! core**: [`Bucket::try_acquire`] is a single `compare_exchange_weak` on a
//! packed atomic word, allocation-free, with lazy refill from the monotonic
//! clock and cache-line alignment against false sharing. The public surface —
//! [`Bucket`], [`BucketConfig`], [`Decision`], [`BucketError`], and the
//! [`TokenBucket`] trait — is unchanged from the `0.2` foundation. The
//! no-over-grant invariant is defended by `loom` model checking, a multi-thread
//! stress test, and `proptest`.
//!
//! ```
//! # #[cfg(feature = "clock")] {
//! use better_bucket::Bucket;
//!
//! // 100 tokens per second, capacity 100.
//! let bucket = Bucket::per_second(100);
//!
//! if bucket.try_acquire(1) {
//!     // allowed — do the work
//! } else {
//!     // denied — shed load / return 429 / back off
//! }
//! # }
//! ```
//!
//! The bucket reads time from [`clock-lib`](https://crates.io/crates/clock-lib);
//! the `clock` feature (on by default) provides it and implies `std`. A bare
//! `no_std` build (`default-features = false`) currently exposes only
//! [`VERSION`] — the no_std-capable, caller-driven core lands with the lock-free
//! rewrite in `0.3`.
//!
//! ## Design goals
//!
//! - **Lock-free acquire.** One `compare_exchange_weak` on a packed atomic
//!   word; no `Mutex`, no parking on the hot path.
//! - **Allocation-free steady state.** A bucket is a small, cache-line-aligned
//!   value with no heap tail; acquiring never allocates.
//! - **Lazy refill.** Tokens accrue from elapsed monotonic time on access — no
//!   timer thread, no wakeups, no watts burned while idle.
//! - **Overflow-safe.** Every refill and capacity computation is checked or
//!   saturating; a hostile request count or a multi-day idle gap can neither
//!   wrap the counter nor over-fill the bucket.
//! - **`no_std`-capable.** The core runs without the standard library; the
//!   caller drives time when `std` is disabled.
//!
//! ## Feature flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `std`   | yes     | Standard library. Off → `no_std`, caller drives time. |
//! | `clock` | yes     | Pluggable [`clock-lib`](https://crates.io/crates/clock-lib) time source plus a mockable clock for deterministic tests. |

// `no_std` for the library build when `std` is off, but always link `std` under
// `test` so the unit-test harness (and dev-dependencies) have what they need.
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![deny(warnings)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(unused_must_use)]
#![deny(unused_results)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::todo)]
#![deny(clippy::unimplemented)]
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]
#![deny(clippy::unreachable)]
#![deny(clippy::undocumented_unsafe_blocks)]

// The token-bucket surface requires a clock to read time and a `Mutex` for the
// simple implementation; both are gated on `clock` (which implies `std`). The
// no_std, caller-driven core arrives with the lock-free rewrite in 0.3.
#[cfg(feature = "clock")]
mod bucket;
#[cfg(feature = "clock")]
mod config;
#[cfg(feature = "clock")]
mod decision;
#[cfg(feature = "clock")]
mod error;

#[cfg(feature = "clock")]
pub use crate::bucket::{Bucket, TokenBucket};
#[cfg(feature = "clock")]
pub use crate::config::BucketConfig;
#[cfg(feature = "clock")]
pub use crate::decision::Decision;
#[cfg(feature = "clock")]
pub use crate::error::BucketError;

/// The version of this crate, taken from `Cargo.toml` at compile time.
///
/// Exposed so a consumer can report the exact `better-bucket` build it links
/// against — useful in diagnostics and version-skew checks across a dependency
/// tree.
///
/// # Examples
///
/// ```
/// // Reports the current 0.x series and carries a major.minor.patch core.
/// let version = better_bucket::VERSION;
/// assert!(version.starts_with("0.3"));
/// assert_eq!(version.split('.').count(), 3);
/// ```
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn test_version_is_well_formed_semver() {
        // A `major.minor.patch` core with no empty components.
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "expected major.minor.patch, got {VERSION}");
        assert!(parts.iter().all(|part| !part.is_empty()));
    }
}
