//! Admission control: rate-limit requests and answer each denial with a
//! `Retry-After` hint — the canonical reason to reach for a token bucket.
//!
//! `acquire` returns a [`Decision`]: on `Allowed` you serve the request, on
//! `Denied` you have the exact wait to put in a `Retry-After` header. Run with
//! `cargo run --example admission_control`.

use better_bucket::{Bucket, Decision};

/// What a handler would return for one request.
enum Response {
    Ok,
    TooManyRequests { retry_after_secs: f64 },
}

/// The gate in front of a handler: take one token, or reject with a hint.
fn admit(limiter: &Bucket) -> Response {
    match limiter.acquire(1) {
        Decision::Allowed => Response::Ok,
        Decision::Denied { retry_after } => Response::TooManyRequests {
            retry_after_secs: retry_after.as_secs_f64(),
        },
        // `Decision` is non-exhaustive; treat anything new as a denial.
        _ => Response::TooManyRequests {
            retry_after_secs: 1.0,
        },
    }
}

fn main() {
    // Allow 3 requests per second, with a burst ceiling of 3.
    let limiter = Bucket::per_second(3);

    // A spike of five requests arrives at once. The first three are served from
    // the burst; the rest are shed with a wait derived from the refill rate.
    for id in 1..=5 {
        match admit(&limiter) {
            Response::Ok => println!("request {id}: 200 OK"),
            Response::TooManyRequests { retry_after_secs } => {
                println!(
                    "request {id}: 429 Too Many Requests (Retry-After: {retry_after_secs:.3}s)"
                );
            }
        }
    }
}
