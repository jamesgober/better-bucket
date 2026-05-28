//! Tier-2 configuration with the builder: a large burst ceiling, a slow
//! sustained refill, and a custom initial fill.
//!
//! Run with `cargo run --example builder`.

use std::time::Duration;

use better_bucket::{Bucket, BucketError};

fn main() -> Result<(), BucketError> {
    // Hold up to 1000 tokens (burst), refill 50 per second, start empty.
    let bucket = Bucket::builder()
        .capacity(1000)
        .refill(50, Duration::from_secs(1))
        .initial(0)
        .build()?;

    println!("capacity (burst ceiling): {}", bucket.capacity());
    println!("available at start:       {}", bucket.available());
    println!(
        "refill amount:            {}",
        bucket.config().refill_amount()
    );

    // An unworkable configuration is rejected at build time.
    let rejected = Bucket::builder().capacity(0).build();
    println!("zero-capacity build:      {rejected:?}");

    Ok(())
}
