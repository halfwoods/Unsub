//! `push` and `frame_at` must not allocate once the analyzer exists: the engine calls `push` from
//! its audio callback. This test binary has a counting allocator of its own.

mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use antistreaming_visualize::Analyzer;
use support::*;

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

fn allocations_during(work: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|counting| counting.set(true));
    work();
    COUNTING.with(|counting| counting.set(false));
    ALLOCATIONS.with(Cell::get)
}

#[test]
fn pushing_and_asking_for_frames_allocate_nothing_after_warm_up() {
    for (rate, channels) in [(44_100, 2u16), (48_000, 1), (96_000, 6)] {
        let mono = noise(8.0, rate, 3);
        let mut signal = Vec::new();
        for sample in &mono {
            signal.extend(std::iter::repeat_n(*sample, usize::from(channels)));
        }
        // Clicks too, so the beat and tempo detectors have something to do.
        let beats = with_bursts(8.0, rate, &beat_times(120.0, 0.1, 8.0), &kick(rate));
        for (frame, beat) in signal.chunks_mut(usize::from(channels)).zip(&beats) {
            frame[0] += beat;
        }
        let mut analyzer = Analyzer::new(rate, channels);
        let block = 480 * usize::from(channels);
        let (warm_up, steady) = signal.split_at(3 * rate as usize * usize::from(channels));
        for (n, chunk) in warm_up.chunks(block).enumerate() {
            analyzer.push(chunk, secs((n * 480) as f64 / f64::from(rate)));
        }
        let started = 3.0;

        let allocated = allocations_during(|| {
            for (n, chunk) in steady.chunks(block).enumerate() {
                let at = started + (n * 480) as f64 / f64::from(rate);
                analyzer.push(chunk, secs(at));
                std::hint::black_box(analyzer.frame_at(secs(at - 0.1)));
            }
        });

        assert_eq!(allocated, 0, "{rate} Hz, {channels} channels");
    }
}
