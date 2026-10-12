//! Stage bench: placement (packed clusters -> HAD sites).
//!
//! Deterministic: the placer uses no RNG (fixed initial placement +
//! deterministic annealing order); the gold lock depends on this.
//! `timing_weight = 0.75` matches what `helion qor` runs.
//! Run: `cargo bench -p helion-place`

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use helion_device::Device;
use helion_pack::Packed;
use helion_place::{PlaceOpts, place_with};
use std::path::PathBuf;
use std::time::Duration;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

struct Fixture {
    id: &'static str,
    packed: Packed,
    samples: usize,
}

fn fixtures() -> (Device, Vec<Fixture>) {
    let dev = Device::load_part("HL10T-C32-1").expect("HAD HL10T-C32-1");
    let mut out = Vec::new();
    for (id, file, samples) in [("counter", "counter.sv", 30), ("ibex", "ysyx_ibex.sv", 10)] {
        let design = helion_sv::synth_sv_path(&example(file)).expect("synth failed");
        let packed = helion_pack::pack(&design, &dev).expect("pack failed");
        out.push(Fixture {
            id,
            packed,
            samples,
        });
    }
    (dev, out)
}

fn bench_place(c: &mut Criterion) {
    let (dev, fixtures) = fixtures();
    let mut group = c.benchmark_group("place");
    group.measurement_time(Duration::from_secs(60));
    for f in &fixtures {
        group.sample_size(f.samples);
        group.bench_with_input(BenchmarkId::from_parameter(f.id), &f.packed, |b, packed| {
            b.iter(|| {
                place_with(
                    black_box(packed),
                    black_box(&dev),
                    black_box(PlaceOpts {
                        timing_weight: 0.75,
                        seed: 0,
                    }),
                )
                .unwrap()
            })
        });
    }
    group.finish();
}

criterion_group!(place_benches, bench_place);
criterion_main!(place_benches);
