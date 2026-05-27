//! Allocation audit for the acquire path.
//!
//! The hot path must never allocate. This test installs a counting global
//! allocator, warms the bucket up, then asserts that a long run of
//! `try_acquire` / `acquire` / `available` calls performs zero allocations.
//!
//! Gated on `clock`: without it there is no `Bucket` to exercise.

#![cfg(feature = "clock")]
#![allow(clippy::unwrap_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use better_bucket::Bucket;
use clock_lib::ManualClock;

/// Counts allocations while delegating to the system allocator.
struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards directly to the system allocator with the same
// arguments, so the allocator contract is upheld unchanged; the only added
// behaviour is a relaxed counter increment, which has no bearing on soundness.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `layout` is forwarded unchanged to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr`/`layout` originate from `System.alloc` above and are
        // forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `layout` is forwarded unchanged to the system allocator.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `ptr`/`layout`/`new_size` originate from and satisfy the
        // system allocator's contract and are forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn test_acquire_path_does_not_allocate() {
    // Build and warm up entirely before measuring; construction and the first
    // clock reads may allocate, the steady-state acquire path must not.
    let clock = Arc::new(ManualClock::new());
    let bucket = Bucket::per_second(1_000).with_clock(Arc::clone(&clock));
    for _ in 0..100 {
        let _ = bucket.try_acquire(1);
    }
    clock.advance(Duration::from_secs(1));

    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let mut granted = 0u64;
    for _ in 0..100_000 {
        if bucket.try_acquire(1) {
            granted += 1;
        }
        let _ = bucket.acquire(1);
        let _ = bucket.available();
        clock.advance(Duration::from_millis(1));
    }
    let after = ALLOCATIONS.load(Ordering::Relaxed);

    assert_eq!(
        after - before,
        0,
        "acquire path allocated {} time(s)",
        after - before
    );
    // Sanity: the loop actually did work, so the assertion above is meaningful.
    assert!(granted > 0);
}
