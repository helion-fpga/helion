# Perf budget proposal (not yet enforced)

Measured 2026-10-10 on the `wip/perf-harness` branch, release profile,
Linux x86_64, 2-CPU VM. Source: `cargo run -p helion-cli --release --example
perf_profile` (per-stage wall / heap-allocated bytes / allocation count /
peak RSS) plus the new criterion benches.

## Measured baseline (ibex scale: `examples/ysyx_ibex.sv`, 15,450 cells)

| stage  | wall_ms | alloc_bytes | alloc_count | peak_rss_kb |
|--------|--------:|------------:|------------:|------------:|
| synth  | 2129    | 1,217,937,364 | 24,212,149 | 70,176 |
| pack   | 31      | 11,415,728  | 36,825      | 70,176 |
| place  | 92      | 10,268,839  | 78,838      | 70,176 |
| route  | 11      | 4,061,114   | 24,801      | 70,176 |
| bitgen | 2       | 1,070,461   | 382         | 70,176 |
| sta    | 21      | 3,212,510   | 5           | 70,176 |

Synth sub-split (from `hang_diag`): parse 1005 ms, `synth_rtl` 1056 ms
(`own_ms` 820 / `stitch_ms` 47 / `hash_ms` 2).

Gold corpus (`examples/counter.sv`, 9 cells): synth 16 ms / 42 KB / 714
allocs; every other stage < 1 ms.

## Proposed gate (standards §7)

Two layers, matching the existing QoR-gate discipline (a move updates the
table in the same commit, with a reason):

**A. `qor.rs` wall-time budgets on the gold corpus** (counter/blinky/hier):

| stage | budget_ms | measured (counter) |
|-------|----------:|-------------------:|
| synth | 50  | 16  |
| pack  | 10  | <1  |
| place | 50  | <1  |
| route | 20  | <1  |
| sta   | 20  | <1  |
| bitgen| 20  | <1  |

**B. Criterion baselines on ibex scale** (`cargo bench -- --save-baseline main`,
CI compares vs `main`, >10% regression fails):

| bench        | budget_ms | measured |
|--------------|----------:|---------:|
| synth/ibex   | 4000 | 2129 |
| pack/ibex    | 100  | 31   |
| place/ibex   | 300  | 92   |
| route/ibex   | 100  | 11   |
| sta/ibex     | 100  | 21   |
| bitgen/ibex  | 50   | 2    |

**C. Allocation tripwires** (counting-allocator data from `perf_profile`):
- `synth/ibex` alloc_bytes ≤ 2.5 GB (measured 1.22 GB) — deliberately loose
  until the elaboration arena work lands, then tighten toward ~100 MB.
- 2× peak-RSS jump on any stage fails review (standards §2).

## Notes

- Budgets carry ~2–3× headroom over this VM's measurements for CI machine
  variance. Re-baseline on the CI runner before enforcing.
- `parse/*` vs `elaborate/*` benches exist so frontend vs lowering
  regressions are attributable.
- All benches are deterministic (no RNG on any measured path); the gold
  `WNS_PS=9640` lock is unaffected.
