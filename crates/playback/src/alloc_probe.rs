//! A counting allocator for tests that must show audio-thread code never allocates. Only the
//! thread running [`allocations_during`] is counted.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

fn note() {
    if COUNTING.with(Cell::get) {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
    }
}

// SAFETY: every method defers to the system allocator; the counting touches only thread-locals
// that need no allocation of their own.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(pointer, layout, new_size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

pub(crate) fn allocations_during(work: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|counting| counting.set(true));
    work();
    COUNTING.with(|counting| counting.set(false));
    ALLOCATIONS.with(Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_sees_allocations_on_its_own_thread_only() {
        assert_eq!(allocations_during(|| {}), 0);
        assert!(allocations_during(|| drop(std::hint::black_box(vec![0_u8; 16]))) > 0);
    }
}
