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
//! ## Guarantees
//!
//! The safety contract holds under any concurrent interleaving, adversarial
//! input, and extreme uptime: **no panic, no wrap, no over-grant, and tokens
//! always within `[0, capacity]`.** It is defended by `loom` model checking, a
//! multi-thread stress test, an allocation audit, an adversarial/edge suite, and
//! `proptest`. `try_acquire` is a single `compare_exchange_weak` on a packed
//! atomic word — allocation-free, cache-line aligned, with a division-free
//! fixed-point refill. The bucket's own accounting measures a few nanoseconds;
//! end-to-end `try_acquire` is bounded by the monotonic clock read (see
//! `docs/BENCHMARKS.md`).
//!
//! Token bucket is the crate's sole algorithm by design — leaky-bucket and
//! sliding-window limiting live in the downstream `rate-net` crate.
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
//! ## Two front doors
//!
//! - [`Bucket`] reads time from an injected [`clock-lib`](https://crates.io/crates/clock-lib)
//!   clock (the OS monotonic clock by default, a `ManualClock` in tests). It
//!   needs the `clock` feature, which is on by default and implies `std`.
//! - [`BucketCore`] is the same engine with the caller supplying the time as a
//!   `Duration` since an epoch of its choosing. It needs only `core` and a
//!   64-bit atomic, so it is the `no_std` path:
//!
//! ```
//! use better_bucket::{BucketConfig, BucketCore};
//! use core::time::Duration;
//!
//! let config = BucketConfig::new(10, 10, Duration::from_secs(1), 10)?;
//! let bucket = BucketCore::new(config, Duration::ZERO);
//! assert!(bucket.try_acquire_at(10, Duration::ZERO));
//! assert_eq!(bucket.available_at(Duration::from_millis(500)), 5);
//! # Ok::<(), better_bucket::BucketError>(())
//! ```
//!
//! ## Design goals
//!
//! - **Lock-free acquire.** One `compare_exchange_weak` on one atomic word; no
//!   `Mutex`, no parking on the hot path.
//! - **Allocation-free steady state.** A bucket is a small, cache-line-aligned
//!   value with no heap tail; acquiring never allocates.
//! - **Lazy refill.** Tokens accrue from elapsed monotonic time on access — no
//!   timer thread, no wakeups, no watts burned while idle.
//! - **Overflow-safe.** Every refill and capacity computation is checked or
//!   saturating; a hostile request count or a multi-day idle gap can neither
//!   wrap the counter nor over-fill the bucket.
//! - **`no_std`-capable.** [`BucketCore`], [`BucketConfig`], [`Decision`], and
//!   [`BucketError`] need only `core`; the caller drives time.
//!
//! ## Feature flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `std`   | yes     | Standard library. Off → `no_std`; [`BucketCore`] and the config/decision/error types remain. |
//! | `clock` | yes     | [`Bucket`] and the [`clock-lib`](https://crates.io/crates/clock-lib) time source, plus a mockable clock for deterministic tests. Implies `std`; brings `error_forge::ForgeError` for [`BucketError`]. |

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
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
// Without `clock`, links to `Bucket` and `TokenBucket` in shared docs have no
// target; the items they describe are simply absent from that build.
#![cfg_attr(not(feature = "clock"), allow(rustdoc::broken_intra_doc_links))]

// The unit tests use `std` (threads, `Arc`, `format!`) even when the library
// itself is built `no_std`.
#[cfg(all(test, not(feature = "std")))]
extern crate std;

#[cfg(feature = "clock")]
mod bucket;
#[cfg(feature = "clock")]
mod builder;
mod config;
mod decision;
// The lock-free engine needs a native 64-bit atomic.
#[cfg(target_has_atomic = "64")]
mod engine;
mod error;

#[cfg(feature = "clock")]
#[cfg_attr(docsrs, doc(cfg(feature = "clock")))]
pub use crate::bucket::{Bucket, TokenBucket};
#[cfg(feature = "clock")]
pub use crate::builder::BucketBuilder;
pub use crate::config::BucketConfig;
pub use crate::decision::Decision;
#[cfg(target_has_atomic = "64")]
pub use crate::engine::BucketCore;
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
/// // Reports the crate version as a `major.minor.patch` string.
/// let version = better_bucket::VERSION;
/// assert!(version.starts_with("1."));
/// assert_eq!(version.split('.').count(), 3);
/// ```
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;
    use std::vec::Vec;

    #[test]
    fn test_version_is_well_formed_semver() {
        // A `major.minor.patch` core with no empty components.
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "expected major.minor.patch, got {VERSION}");
        assert!(parts.iter().all(|part| !part.is_empty()));
    }
}
