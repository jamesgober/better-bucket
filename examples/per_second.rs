//! The Tier-1 common case: a per-second rate limiter.
//!
//! Run with `cargo run --example per_second`.

use better_bucket::Bucket;

fn main() {
    // Allow 5 requests per second, with a burst ceiling of 5.
    let limiter = Bucket::per_second(5);

    // Fire eight requests in a tight loop. The first five drain the bucket;
    // the rest are denied, because almost no time has passed to refill it.
    for request in 1..=8 {
        if limiter.try_acquire(1) {
            println!("request {request}: allowed");
        } else {
            println!("request {request}: denied (rate limited)");
        }
    }
}
