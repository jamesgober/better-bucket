//! Burst handling: a bucket absorbs a spike up to its capacity, then throttles
//! back to the sustained refill rate.
//!
//! Uses a `ManualClock` so the timing is deterministic — no `sleep`. Run with
//! `cargo run --example burst`.

use std::sync::Arc;
use std::time::Duration;

use better_bucket::Bucket;
use clock_lib::ManualClock;

fn main() {
    let clock = Arc::new(ManualClock::new());

    // Capacity 10 is the burst ceiling; it refills 10 tokens per second.
    let bucket = Bucket::per_second(10).with_clock(Arc::clone(&clock));

    // A client can spend the whole burst at once.
    println!("burst of 10 at once: {}", bucket.try_acquire(10));
    println!("one more right away:  {}", bucket.try_acquire(1));

    // After a quarter second, a quarter of the rate has accrued.
    clock.advance(Duration::from_millis(250));
    println!("available after 250ms: {}", bucket.available());

    // After a full second, the burst is fully restored.
    clock.advance(Duration::from_millis(750));
    println!("available after 1s:    {}", bucket.available());
}
