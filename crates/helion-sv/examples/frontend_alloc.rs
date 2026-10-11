//! Allocation + wall-clock harness for the INTERNAL SystemVerilog frontend
//! (preprocess + tokenize + parse_one_module), over ibex.
//!
//! Run: `cargo run --release -p helion-sv --example frontend_alloc`
//!
//! Deterministic: no RNG on this path; same input, same counts.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
// Size histogram: bucket[i] = count of allocs with layout.size() == i, i < 2048.
static HIST: [AtomicU64; 2048] = [const { AtomicU64::new(0) }; 2048];
static BIG: AtomicU64 = AtomicU64::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        if layout.size() < 2048 {
            HIST[layout.size()].fetch_add(1, Ordering::Relaxed);
        } else {
            BIG.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn main() {
    let src = std::fs::read_to_string("examples/ysyx_ibex.sv").expect("ibex example missing");
    // Warm the FS/page cache out of the measurement; allocation counters start here.
    let _ = helion_sv::list_sv_modules_origin("module m; endmodule", "warm");
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    let t = Instant::now();
    let mods = helion_sv::list_sv_modules_origin(&src, "ibex").expect("parse failed");
    let dt = t.elapsed();
    println!(
        "frontend mods={} wall_ms={} allocs={} alloc_bytes={}",
        mods.len(),
        dt.as_millis(),
        ALLOCS.load(Ordering::Relaxed),
        BYTES.load(Ordering::Relaxed)
    );
    // Top-20 allocation sizes.
    let mut v: Vec<(usize, u64)> = (0..2048)
        .map(|i| (i, HIST[i].load(Ordering::Relaxed)))
        .filter(|&(_, n)| n > 0)
        .collect();
    v.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    println!("top sizes (size: count):");
    for (s, n) in v.iter().take(20) {
        println!("  {s}: {n}");
    }
    println!("big(>=2048): {}", BIG.load(Ordering::Relaxed));
}
