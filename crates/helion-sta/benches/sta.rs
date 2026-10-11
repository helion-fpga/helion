//! Stage bench: static timing analysis on routed designs.
//!
//! Deterministic: pure graph propagation, no RNG. Uses a 10 ns `clk`
//! like `helion qor` does when no XDC clock is present.
//! Run: `cargo bench -p helion-sta`

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use helion_ir::Design;
use helion_route::Routed;
use helion_sta::{Clock, create_clock, report_timing_routed};
use std::path::PathBuf;
use std::time::Duration;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

struct Fixture {
    id: &'static str,
    design: Design,
    routed: Routed,
    samples: usize,
}

fn fixtures() -> Vec<Fixture> {
    let dev = helion_device::Device::load_part("HL10T-C32-1").expect("HAD HL10T-C32-1");
    let mut out = Vec::new();
    for (id, file, samples) in [("counter", "counter.sv", 50), ("ibex", "ysyx_ibex.sv", 10)] {
        let design = helion_sv::synth_sv_path(&example(file)).expect("synth failed");
        let packed = helion_pack::pack(&design, &dev).expect("pack failed");
        let max_iters = if packed.lutffs.len() > 2000 { 24 } else { 8 };
        let placed = helion_place::place_with(&packed, &dev, helion_place::PlaceOpts::default())
            .expect("place failed");
        let routed = helion_route::route_with(
            &placed,
            &dev,
            helion_route::RouteOpts {
                max_iters,
                extra_hops: 0,
            },
        )
        .expect("route failed");
        out.push(Fixture {
            id,
            design,
            routed,
            samples,
        });
    }
    out
}

fn clocks() -> Vec<Clock> {
    let mut clks = Vec::new();
    create_clock(&mut clks, "clk", 10_000, "clk");
    clks
}

fn bench_sta(c: &mut Criterion) {
    let fixtures = fixtures();
    let mut group = c.benchmark_group("sta");
    group.measurement_time(Duration::from_secs(30));
    for f in &fixtures {
        group.sample_size(f.samples);
        let clks = clocks();
        group.bench_with_input(
            BenchmarkId::from_parameter(f.id),
            &(&f.design, &f.routed),
            |b, (design, routed)| {
                b.iter(|| {
                    report_timing_routed(black_box(design), black_box(routed), black_box(&clks))
                        .unwrap()
                })
            },
        );
    }
    group.finish();
}

criterion_group!(sta_benches, bench_sta);
criterion_main!(sta_benches);
