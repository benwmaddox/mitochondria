//! Counts heap allocations per thread, so tests and benchmarks can check that a request allocates nothing.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! { static N: Cell<usize> = const { Cell::new(0) }; }
fn bump() { let _ = N.try_with(|n| n.set(n.get() + 1)); }

pub struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { bump(); unsafe { System.alloc(l) } }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 { bump(); unsafe { System.alloc_zeroed(l) } }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 { bump(); unsafe { System.realloc(p, l, n) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
}

/// Heap allocations made by this thread so far.
pub fn allocs() -> usize { N.try_with(|n| n.get()).unwrap_or(0) }
