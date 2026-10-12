//! Instrumented full-flow profiler: per-stage wall time, heap allocations,
//! and peak RSS for the exact `helion qor` pipeline.
//!
//! Run (release!):
//! `cargo run -q -p helion-cli --release --example perf_profile -- [design.sv ...]`
//! Defaults: `examples/counter.sv` + `examples/ysyx_ibex.sv`.
//!
//! Measurement, not optimization: every stage is called with the same
//! options the CLI uses. The counting allocator attributes each allocation
//! to the stage running when it happens, so the `alloc_b`/`alloc_n` columns
//! are the allocator-traffic signal the quality standards care about
//! (standards §1: no allocation inside anneal/route/STA inner loops).

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn alloc_snapshot() -> (usize, usize) {
    (
        ALLOC_BYTES.load(Ordering::SeqCst),
        ALLOC_COUNT.load(Ordering::SeqCst),
    )
}

fn peak_rss_kb() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let designs: Vec<PathBuf> = if args.is_empty() {
        vec![
            root.join("examples/counter.sv"),
            root.join("examples/ysyx_ibex.sv"),
        ]
    } else {
        args.iter().map(PathBuf::from).collect()
    };

    let dev = helion_device::Device::load_part("HL10T-C32-1").expect("HAD HL10T-C32-1");

    println!(
        "{:<28} {:>10} {:>12} {:>10} {:>10}",
        "stage", "wall_ms", "alloc_b", "alloc_n", "rss_kb"
    );
    for path in &designs {
        println!("--- {}", path.display());
        // Stage 1: synth (parse + AIG elaboration + FlowMap).
        let t = Instant::now();
        let (b0, n0) = alloc_snapshot();
        let design = helion_sv::synth_sv_path(path).expect("synth failed");
        stage("synth", t, b0, n0, design.cells.len());

        // Stage 2: pack.
        let t = Instant::now();
        let (b0, n0) = alloc_snapshot();
        let packed = helion_pack::pack(&design, &dev).expect("pack failed");
        stage("pack", t, b0, n0, packed.lutffs.len());

        // Stage 3: place (timing_weight 0.75, like `helion qor`).
        let t = Instant::now();
        let (b0, n0) = alloc_snapshot();
        let placed = helion_place::place_with(
            &packed,
            &dev,
            helion_place::PlaceOpts {
                timing_weight: 0.75, seed: 0 },
        )
        .expect("place failed");
        stage("place", t, b0, n0, placed.lutff_sites.len());

        // Stage 4: route (CLI iter rule).
        let max_iters = if packed.lutffs.len() > 2000 { 24 } else { 8 };
        let t = Instant::now();
        let (b0, n0) = alloc_snapshot();
        let routed = helion_route::route_with(
            &placed,
            &dev,
            helion_route::RouteOpts {
                max_iters,
                extra_hops: 0,
            },
        )
        .expect("route failed");
        stage("route", t, b0, n0, routed.imux.len());

        // Stage 5: bitgen.
        let t = Instant::now();
        let (b0, n0) = alloc_snapshot();
        let bits = helion_bits::bitgen(&dev, &routed).expect("bitgen failed");
        stage("bitgen", t, b0, n0, bits.packets.len());

        // Stage 6: STA on the routed design (10 ns clk, like `helion qor`).
        let mut clks = Vec::new();
        helion_sta::create_clock(&mut clks, "clk", 10_000, "clk");
        let t = Instant::now();
        let (b0, n0) = alloc_snapshot();
        let timing = helion_sta::report_timing_routed(&design, &routed, &clks).expect("STA failed");
        stage("sta", t, b0, n0, timing.wns_ps as usize);
    }
}

fn stage(name: &str, t: Instant, b0: usize, n0: usize, summary: usize) {
    let (b1, n1) = alloc_snapshot();
    println!(
        "{:<28} {:>10} {:>12} {:>10} {:>10}  // {}",
        name,
        t.elapsed().as_millis(),
        b1 - b0,
        n1 - n0,
        peak_rss_kb(),
        summary,
    );
}
