//! Test builds only: a global allocator that counts the bytes live on each thread, so a test can measure what a copy of a world
//! holds on the heap (`sim::tests::clone_size_model_matches_a_measured_clone_at_5000_households`). Every request goes to the system
//! allocator. The count is a thread-local cell, so a test sees only the allocations made on its own thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = LIVE.try_with(|live| live.set(live.get() + layout.size() as isize));
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = LIVE.try_with(|live| live.set(live.get() - layout.size() as isize));
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// The bytes live on this thread, as counted: allocations made here less the bytes freed here.
pub fn live_bytes() -> isize {
    LIVE.with(Cell::get)
}
