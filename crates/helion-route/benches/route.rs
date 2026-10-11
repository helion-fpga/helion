//! Stage bench: PathFinder routing (placed clusters -> routed nets).
//!
//! Deterministic: no RNG on this path. `max_iters` follows the CLI rule
//! (24 when packed LUTFFs > 2000, else 8); `extra_hops = 0`.
//! Run: `cargo bench -p helion-route`

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use helion_device::Device;
use helion_place::Placed;
use helion_route::{RouteOpts, route_with};
use std::path::PathBuf;
use std::time::Duration;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

struct Fixture {
    id: &'static str,
    placed: Placed,
    max_iters: u32,
    samples: usize,
}

fn fixtures() -> (Device, Vec<Fixture>) {
    let dev = Device::load_part("HL10T-C32-1").expect("HAD HL10T-C32-1");
    let mut out = Vec::new();
    for (id, file, samples) in [("counter", "counter.sv", 30), ("ibex", "ysyx_ibex.sv", 10)] {
        let design = helion_sv::synth_sv_path(&example(file)).expect("synth failed");
        let packed = helion_pack::pack(&design, &dev).expect("pack failed");
        let max_iters = if packed.lutffs.len() > 2000 { 24 } else { 8 };
        let placed = helion_place::place_with(&packed, &dev, helion_place::PlaceOpts::default())
            .expect("place failed");
        out.push(Fixture {
            id,
            placed,
            max_iters,
            samples,
        });
    }
    (dev, out)
}

fn bench_route(c: &mut Criterion) {
    let (dev, fixtures) = fixtures();
    let mut group = c.benchmark_group("route");
    group.measurement_time(Duration::from_secs(60));
    for f in &fixtures {
        group.sample_size(f.samples);
        let max_iters = f.max_iters;
        group.bench_with_input(BenchmarkId::from_parameter(f.id), &f.placed, |b, placed| {
            b.iter(|| {
                route_with(
                    black_box(placed),
                    black_box(&dev),
                    black_box(RouteOpts {
                        max_iters,
                        extra_hops: 0,
                    }),
                )
                .unwrap()
            })
        });
    }
    group.finish();
}

criterion_group!(route_benches, bench_route);
criterion_main!(route_benches);
