//! The caller-driven core: a token bucket with no clock, as a `no_std` firmware
//! loop or a deterministic simulation would use it.
//!
//! `BucketCore` takes the time as a `Duration` since an epoch you choose — here
//! a simulated millisecond tick counter. Nothing reads the OS clock, so the run
//! is identical every time. Run with `cargo run --example caller_driven`; the
//! same code compiles with `default-features = false`.

use std::time::Duration;

use better_bucket::{BucketConfig, BucketCore, Decision};

fn main() {
    // 5 messages per second, bursts of up to 5, starting full at tick 0.
    let config = match BucketConfig::new(5, 5, Duration::from_secs(1), 5) {
        Ok(config) => config,
        Err(err) => {
            println!("invalid configuration: {err}");
            return;
        }
    };
    let uplink = BucketCore::new(config, Duration::ZERO);

    // A sensor wants to send every 50 ms for one second: 20 attempts, of which
    // the budget allows the burst of 5 plus the refill over the second.
    let mut sent = 0;
    for tick_ms in (0..1_000).step_by(50) {
        let now = Duration::from_millis(tick_ms);
        match uplink.acquire_at(1, now) {
            Decision::Allowed => sent += 1,
            Decision::Denied { retry_after } => {
                println!("t={tick_ms:>4} ms  held back, next slot in {retry_after:?}");
            }
            _ => {}
        }
    }
    println!("sent {sent} of 20 in one second");
    assert_eq!(sent, 9); // 5 burst + refill at 200, 400, 600, 800 ms

    // A large frame that a stream of small ones could starve: reserve its
    // place, then wait out the returned delay before transmitting.
    let now = Duration::from_millis(1_000);
    if let Some(wait) = uplink.reserve_at(5, Duration::from_secs(2), now) {
        println!("frame of 5 reserved; transmit in {wait:?}");
    }
}
