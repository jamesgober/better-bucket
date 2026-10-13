//! Property tests for the clock-free engine, `BucketCore`.
//!
//! The caller supplies every reading, so these properties can drive time
//! anywhere — forwards in arbitrary steps, backwards, or not at all — across
//! arbitrary rates, and check the contracts the engine promises:
//!
//! - **No over-grant**, in every window: the tokens granted between two
//!   decisions never exceed the capacity plus the refill accrued between them
//!   (plus one token of slack for the sub-nanosecond clock rounding). With
//!   readings that step backwards, the whole run stays within the initial fill
//!   plus the refill up to the latest reading.
//! - **Exact conservation** without refill: every token is either granted,
//!   refunded back, or still available.
//! - **`available_at` is the exact grant boundary**, and **the retry hint and
//!   `time_until_at` are honest and agree**.
//! - **Reservations never commit more** than the bucket covers by the time the
//!   reservation's wait ends.
//!
//! Runs in every build configuration, including `--no-default-features`.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use better_bucket::{BucketConfig, BucketCore, Decision};
use proptest::prelude::*;

/// A configuration plus its exact refill rate, for the bound arithmetic.
#[derive(Debug, Clone, Copy)]
struct Setup {
    capacity: u32,
    amount: u32,
    period_ns: u64,
    initial: u32,
}

impl Setup {
    fn config(self) -> BucketConfig {
        BucketConfig::new(
            self.capacity,
            self.amount,
            Duration::from_nanos(self.period_ns),
            self.initial,
        )
        .unwrap()
    }

    /// Whole tokens accrued over `ns`, rounded up (the generous bound).
    fn accrued(self, ns: u64) -> u128 {
        (u128::from(self.amount) * u128::from(ns)).div_ceil(u128::from(self.period_ns))
    }

    /// Whether `granted` tokens fit within `base` tokens plus the exact refill
    /// over `ns`, allowing only the bucket's documented resolution: a token
    /// may be due up to 4 ns (two units) early, or 2^-28 of a token for a
    /// bucket whose capacity limits its resolution. Exact rational
    /// arithmetic, scaled by 2^28 · period.
    fn within(self, granted: u128, base: u32, ns: u64) -> bool {
        let period = u128::from(self.period_ns);
        let lhs = (granted * period) << 28;
        let exact = u128::from(base) * period + u128::from(self.amount) * u128::from(ns + 4);
        lhs <= (exact << 28) + period
    }
}

/// Every regime the engine distinguishes: small and `u32`-sized capacities
/// (the capacity-limited unit), and rates from one per month to ~4 × 10^18 per
/// second (sub-microsecond periods with huge amounts are the multi-token-unit
/// regime above ~4.3 × 10^9 tokens/second).
fn setups() -> impl Strategy<Value = Setup> {
    (
        prop_oneof![1_u32..=5_000, 1_u32..=u32::MAX],
        prop_oneof![1_u32..=5_000, 1_u32..=u32::MAX],
        prop_oneof![
            1_u64..=999,
            1_000_u64..=10_000_000_000,
            86_400_000_000_000_u64..=30 * 86_400_000_000_000,
        ],
        any::<u32>(),
    )
        .prop_map(|(capacity, amount, period_ns, initial)| Setup {
            capacity,
            amount,
            period_ns,
            initial: if capacity == u32::MAX {
                initial
            } else {
                initial % (capacity + 1)
            },
        })
}

/// Setups whose grants are exact: every rate up to ~4.3 × 10^9 tokens/second.
/// Faster ones are slowed to that limit by lengthening the period.
fn exact_setups() -> impl Strategy<Value = Setup> {
    setups().prop_map(|mut setup| {
        let min_period = (u128::from(setup.amount) * 10).div_ceil(43);
        setup.period_ns = setup
            .period_ns
            .max(u64::try_from(min_period).unwrap_or(u64::MAX));
        setup
    })
}

/// One operation: a raw request size (scaled to the capacity by `request`) and
/// a signed clock step in nanoseconds.
fn ops(backwards: bool) -> impl Strategy<Value = Vec<(u32, i64)>> {
    let step = if backwards {
        -50_000_000_i64..=200_000_000
    } else {
        0_i64..=200_000_000
    };
    proptest::collection::vec((any::<u32>(), step), 1..64)
}

/// A request size between 1 and a quarter of the capacity (most grantable),
/// or occasionally above the capacity (never grantable).
fn request(setup: Setup, raw: u32) -> u32 {
    if raw % 16 == 0 {
        setup.capacity.saturating_add(1 + raw % 7)
    } else {
        1 + raw % (setup.capacity / 4 + 1)
    }
}

fn at(ns: u64) -> Duration {
    Duration::from_nanos(ns)
}

proptest! {
    /// No window of decisions grants more than the capacity plus the refill
    /// accrued across that window.
    #[test]
    fn core_never_over_grants_in_any_window(setup in setups(), steps in ops(false)) {
        let bucket = BucketCore::new(setup.config(), Duration::ZERO);
        let mut now = 0_u64;
        let mut log: Vec<(u64, u32)> = Vec::new(); // (time, tokens granted)
        for (raw, step) in steps {
            let n = request(setup, raw);
            now = now.saturating_add(step.unsigned_abs());
            if bucket.try_acquire_at(n, at(now)) {
                log.push((now, n));
            }
            prop_assert!(bucket.available_at(at(now)) <= setup.capacity);
        }
        // From construction: the initial fill plus the exact refill since
        // t = 0, to within the bucket's resolution.
        let total: u128 = log.iter().map(|&(_, n)| u128::from(n)).sum();
        prop_assert!(setup.within(total, setup.initial, now), "total {total} by {now} ns");
        // Every window [i, j]: capacity plus the exact refill between them.
        for i in 0..log.len() {
            let mut granted = 0_u128;
            for j in i..log.len() {
                granted += u128::from(log[j].1);
                prop_assert!(
                    setup.within(granted, setup.capacity, log[j].0 - log[i].0),
                    "window {}..={}: {} over {} ns", i, j, granted, log[j].0 - log[i].0
                );
            }
        }
    }

    /// Readings that jump backwards (a stale caller, a misbehaving clock) can
    /// only cost tokens: the run never exceeds the initial fill plus the refill
    /// up to the latest reading seen.
    #[test]
    fn core_backward_readings_never_add_tokens(setup in setups(), steps in ops(true)) {
        let bucket = BucketCore::new(setup.config(), Duration::ZERO);
        let mut now = 0_i64;
        let mut latest = 0_u64;
        let mut total = 0_u128;
        for (raw, step) in steps {
            let n = request(setup, raw);
            now = (now + step).max(0);
            let reading = now.unsigned_abs();
            latest = latest.max(reading);
            if bucket.try_acquire_at(n, at(reading)) {
                total += u128::from(n);
            }
            prop_assert!(bucket.available_at(at(reading)) <= setup.capacity);
        }
        prop_assert!(setup.within(total, setup.initial, latest), "total {total} by {latest} ns");
    }

    /// With the clock frozen, tokens are conserved exactly through acquires,
    /// partial grants, and refunds of what was taken.
    #[test]
    fn core_frozen_clock_conserves_every_token(
        setup in exact_setups(),
        steps in proptest::collection::vec((0_u8..3, 1_u32..=600), 0..96),
    ) {
        let t = at(1_234_567);
        let bucket = BucketCore::new(setup.config(), t);
        let mut held = 0_u64;
        for (kind, n) in steps {
            match kind {
                0 => {
                    if bucket.try_acquire_at(n, t) {
                        held += u64::from(n);
                    }
                }
                1 => held += u64::from(bucket.acquire_up_to_at(n, t)),
                _ => {
                    let back = u32::try_from(held.min(u64::from(n))).unwrap();
                    bucket.refund(back);
                    held -= u64::from(back);
                }
            }
            prop_assert_eq!(held + u64::from(bucket.available_at(t)), u64::from(setup.initial));
        }
    }

    /// `available_at` is exactly the largest request that would be granted.
    #[test]
    fn core_available_is_the_exact_grant_boundary(
        setup in setups(),
        drain in 0_u32..=5_000,
        later in 0_u64..=20_000_000_000,
    ) {
        let bucket = BucketCore::new(setup.config(), Duration::ZERO);
        let _ = bucket.acquire_up_to_at(drain, Duration::ZERO);
        let n = bucket.available_at(at(later));
        if n < setup.capacity {
            prop_assert!(!bucket.try_acquire_at(n + 1, at(later)));
        }
        prop_assert!(bucket.try_acquire_at(n, at(later)));
    }

    /// A denial's hint equals `time_until_at`, and waiting it is enough.
    #[test]
    fn core_retry_after_is_honest_and_matches_time_until(
        setup in setups(),
        drain in 0_u32..=5_000,
        n in any::<u32>(),
        start in 0_u64..=5_000_000_000,
    ) {
        let n = 1 + n % setup.capacity;
        let bucket = BucketCore::new(setup.config(), Duration::ZERO);
        let _ = bucket.acquire_up_to_at(drain, at(start));
        let predicted = bucket.time_until_at(n, at(start));
        match bucket.acquire_at(n, at(start)) {
            Decision::Allowed => prop_assert_eq!(predicted, Some(Duration::ZERO)),
            Decision::Denied { retry_after } if retry_after == Duration::MAX => {
                // "Never" (the wait outlasts the clock): `time_until` says None.
                prop_assert_eq!(predicted, None);
            }
            Decision::Denied { retry_after } => {
                prop_assert_eq!(predicted, Some(retry_after));
                let due = at(start) + retry_after;
                prop_assert!(bucket.try_acquire_at(n, due), "waited {:?}, still denied", retry_after);
            }
            _ => prop_assert!(false, "unknown decision"),
        }
    }

    /// Every reservation is covered by the time its wait ends: the tokens
    /// reserved so far never exceed the initial fill plus the refill up to
    /// that moment.
    #[test]
    fn core_reservations_never_over_commit(
        setup in setups(),
        steps in proptest::collection::vec((1_u32..=5_000, 0_u64..=100_000_000), 1..48),
    ) {
        let bucket = BucketCore::new(setup.config(), Duration::ZERO);
        let max_wait = Duration::from_secs(30);
        let mut now = 0_u64;
        let mut committed = 0_u128;
        for (n, step) in steps {
            now += step;
            if let Some(wait) = bucket.reserve_at(n, max_wait, at(now)) {
                prop_assert!(wait <= max_wait);
                committed += u128::from(n);
                let covered_at = now + u64::try_from(wait.as_nanos()).unwrap();
                prop_assert!(
                    committed <= u128::from(setup.initial) + setup.accrued(covered_at) + 1,
                    "reserved {} by {} ns", committed, covered_at
                );
            }
        }
    }
}
