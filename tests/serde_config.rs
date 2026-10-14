//! `serde` support for `BucketConfig`: round-trips, and validation on the way
//! in, so a configuration file cannot construct a bucket `BucketConfig::new`
//! would reject.

#![cfg(feature = "serde")]
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use better_bucket::BucketConfig;

#[test]
fn test_config_round_trips_through_json() {
    let config = BucketConfig::new(500, 100, Duration::from_millis(1_500), 25).unwrap();
    let json = serde_json::to_string(&config).unwrap();
    assert_eq!(
        json,
        r#"{"capacity":500,"refill_amount":100,"refill_period":{"secs":1,"nanos":500000000},"initial":25}"#
    );
    let back: BucketConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, config);
}

#[test]
fn test_deserialize_validates_like_new() {
    for (json, expected) in [
        (
            r#"{"capacity":0,"refill_amount":1,"refill_period":{"secs":1,"nanos":0},"initial":0}"#,
            "bucket capacity must be greater than zero",
        ),
        (
            r#"{"capacity":1,"refill_amount":0,"refill_period":{"secs":1,"nanos":0},"initial":0}"#,
            "refill amount must be greater than zero",
        ),
        (
            r#"{"capacity":1,"refill_amount":1,"refill_period":{"secs":0,"nanos":0},"initial":0}"#,
            "refill period must be greater than zero",
        ),
    ] {
        let err = serde_json::from_str::<BucketConfig>(json).unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[test]
fn test_deserialize_clamps_initial_like_new() {
    let json =
        r#"{"capacity":10,"refill_amount":1,"refill_period":{"secs":1,"nanos":0},"initial":99}"#;
    let config: BucketConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.initial(), 10);
}

#[test]
fn test_deserialize_rejects_unknown_and_missing_fields() {
    let unknown = r#"{"capacity":1,"refill_amount":1,"refill_period":{"secs":1,"nanos":0},"initial":0,"burst":5}"#;
    assert!(serde_json::from_str::<BucketConfig>(unknown).is_err());
    let missing = r#"{"capacity":1,"refill_amount":1,"initial":0}"#;
    assert!(serde_json::from_str::<BucketConfig>(missing).is_err());
}
