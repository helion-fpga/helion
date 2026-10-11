//! Stage bench: SystemVerilog synthesis (parse + AIG elaboration + FlowMap).
//!
//! Deterministic: no RNG anywhere on this path; same input, same Design.
//! Run: `cargo bench -p helion-sv`
//!
//! `parse/*` isolates the sv-parser frontend; `elaborate/*` is the full
//! `synth_sv_path` (what `helion qor` runs). The delta is AIG elaboration —
//! the stage the 10-design benchmark flagged as the slowest on large designs.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use std::path::PathBuf;
use std::time::Duration;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

/// (id, file, criterion sample size). Ibex is ~4 s/iter: keep samples low.
const DESIGNS: &[(&str, &str, usize)] =
    &[("counter", "counter.sv", 30), ("ibex", "ysyx_ibex.sv", 10)];

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    for (id, file, samples) in DESIGNS {
        let src = std::fs::read_to_string(example(file)).expect("example missing");
        group.sample_size(*samples);
        group.bench_with_input(BenchmarkId::from_parameter(id), &src, |b, src| {
            b.iter(|| helion_sv::parse_sv(black_box(src), black_box("bench")).unwrap())
        });
    }
    group.finish();
}

fn bench_elaborate(c: &mut Criterion) {
    let mut group = c.benchmark_group("elaborate");
    group.measurement_time(Duration::from_secs(40));
    for (id, file, samples) in DESIGNS {
        let path = example(file);
        group.sample_size(*samples);
        group.bench_with_input(BenchmarkId::from_parameter(id), &path, |b, path| {
            b.iter(|| helion_sv::synth_sv_path(black_box(path)).unwrap())
        });
    }
    group.finish();
}

/// Internal frontend: the recursive-descent parser + RTL elaboration that
/// `helion qor` actually runs (as opposed to `parse/*`, which measures the
/// external `sv_parser` crate). This is the parser-allocation campaign's
/// target: ~98% of frontend wall on Ibex.
fn bench_parse_internal(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_internal");
    group.measurement_time(Duration::from_secs(40));
    for (id, file, samples) in DESIGNS {
        let src = std::fs::read_to_string(example(file)).expect("example missing");
        group.sample_size(*samples);
        group.bench_with_input(BenchmarkId::from_parameter(id), &src, |b, src| {
            b.iter(|| helion_sv::list_sv_modules_origin(black_box(src), black_box("bench")).unwrap())
        });
    }
    group.finish();
}

criterion_group!(synth_benches, bench_parse, bench_parse_internal, bench_elaborate);
criterion_main!(synth_benches);
