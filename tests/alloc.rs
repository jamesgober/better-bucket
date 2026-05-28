//! Allocation audit for the acquire path.
//!
//! The hot path must never allocate. This installs a global allocator that
//! counts allocations **per thread**, warms every operation the measured loop
//! uses, then asserts that a long steady-state run of `try_acquire` / `acquire`
//! / `available` performs zero allocations *on the measuring thread*.
//!
//! Per-thread counting matters: a global counter would also catch incidental
//! allocations made by the test harness or runtime on other threads, which has
//! nothing to do with the bucket and varies by platform.
//!
//! Gated on `clock`: without it there is no `Bucket` to exercise.

#![cfg(feature = "clock")]
#![allow(clippy::unwrap_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use better_bucket::Bucket;
use clock_lib::ManualClock;

thread_local! {
    // `const` init uses native thread-local storage, so reading or incrementing
    // it never allocates and cannot recurse into the allocator below.
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

#[inline]
fn note_alloc() {
    ALLOCATIONS.with(|c| c.set(c.get() + 1));
}

fn alloc_count() -> u64 {
    ALLOCATIONS.with(Cell::get)
}

/// Counts the current thread's allocations, delegating to the system allocator.
struct Counting;

// SAFETY: every method forwards directly to the system allocator with the same
// arguments, so the allocator contract is upheld unchanged; the only added
// behaviour is a non-allocating per-thread counter increment.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        // SAFETY: `layout` is forwarded unchanged to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr`/`layout` originate from `System.alloc` and are forwarded
        // unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        // SAFETY: `layout` is forwarded unchanged to the system allocator.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_alloc();
        // SAFETY: `ptr`/`layout`/`new_size` satisfy the system allocator's
        // contract and are forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn test_acquire_path_does_not_allocate() {
    let clock = Arc::new(ManualClock::new());
    let bucket = Bucket::per_second(1_000).with_clock(Arc::clone(&clock));

    // Warm up every operation the measured loop performs, so any one-time lazy
    // initialisation in std or the OS happens before the measurement window.
    let mut warm = 0u64;
    for _ in 0..2_000 {
        if bucket.try_acquire(1) {
            warm += 1;
        }
        let _ = bucket.acquire(1);
        let _ = bucket.available();
        clock.advance(Duration::from_millis(1));
    }
    assert!(warm > 0);

    // Steady-state window: the acquire path must allocate nothing here.
    let before = alloc_count();
    let mut granted = 0u64;
    for _ in 0..100_000 {
        if bucket.try_acquire(1) {
            granted += 1;
        }
        let _ = bucket.acquire(1);
        let _ = bucket.available();
        clock.advance(Duration::from_millis(1));
    }
    let allocations = alloc_count() - before;

    assert_eq!(
        allocations, 0,
        "acquire path allocated {allocations} time(s) on the measuring thread"
    );
    assert!(granted > 0); // the loop did real work, so the assertion is meaningful
}
