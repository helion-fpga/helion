//! Stage bench: technology packing (NetlistIR -> Packed LUTFF/IOB/BRAM/MAC).
//!
//! Deterministic: no RNG on this path. Fixtures (elaborated designs +
//! HL10T device) are built once outside the measured region.
//! Run: `cargo bench -p helion-pack`

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use helion_device::Device;
use helion_ir::Design;
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
    samples: usize,
}

fn fixtures() -> (Device, Vec<Fixture>) {
    let dev = Device::load_part("HL10T-C32-1").expect("HAD HL10T-C32-1");
    let mut out = Vec::new();
    for (id, file, samples) in [("counter", "counter.sv", 50), ("ibex", "ysyx_ibex.sv", 10)] {
        let design = helion_sv::synth_sv_path(&example(file)).expect("synth failed");
        out.push(Fixture {
            id,
            design,
            samples,
        });
    }
    (dev, out)
}

fn bench_pack(c: &mut Criterion) {
    let (dev, fixtures) = fixtures();
    let mut group = c.benchmark_group("pack");
    group.measurement_time(Duration::from_secs(30));
    for f in &fixtures {
        group.sample_size(f.samples);
        group.bench_with_input(BenchmarkId::from_parameter(f.id), &f.design, |b, design| {
            b.iter(|| helion_pack::pack(black_box(design), black_box(&dev)).unwrap())
        });
    }
    group.finish();
}

criterion_group!(pack_benches, bench_pack);
criterion_main!(pack_benches);
