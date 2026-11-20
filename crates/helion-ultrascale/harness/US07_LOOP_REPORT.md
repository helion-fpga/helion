# US-07: random-design generator and the 100-design loop, report

Branch: `fm/ultra-bitgen-us07`, base `fm/ultra-bitstream` 07d0d7d.
Status: loop complete, 100/100 designs diffed at tier 1 on xcku3p.
100/100 green was NOT reached; the loop measured exactly why, per
block, with the root causes pinned by experiment. Nothing here claims
Vivado equivalence; tier 2 waits for Vivado, license state UNKNOWN.

## What was built

- `gen_random.py`: seeded generator (seed base 20261119). 100 distinct
  valid xcku3p FASM designs in `generated/`: 50 core (random LUT INITs,
  FF INIT/SRVAL, INT PIPs from real `segbits_int.db` edges, frozen IO
  lines) and 50 bram (core plus one RAMB18 with random INIT/INITP).
  Every feature is resolved against the tier-1 reference before the
  design is written. Seeds recorded in every meta file; the same seed
  produces the same design bytes. Coverage over the 100: 796 LUTs
  (50,944 LUT_INIT bits), 1,466 FF bits, 50 BRAM designs (819,200
  BRAM_CONTENT plus 102,400 BRAM_PARITY bits), 1,110 INT_PIP bits,
  5,500 IO_CONFIG bits (tile-relative).
- `run_100.py`: loop driver on top of the existing `diff.py`
  machinery (`diff_engine.diff_design`). Batches of 10, placement-hash
  verification per design, honest per-design and per-batch runtimes
  in every batch manifest.
- `tests/test_gen_random.py`: 11 new tests (determinism, validity,
  probe blocking). Harness suite: 59 tests pass.
- `gen_golden.py`: `design_ff_locations` accepts per-design
  `ff_locations` so multi-slice designs complete FF defaults on
  their real tiles; frozen goldens unaffected.
- 3 probe designs (`probe_dsp`, `probe_uram`, `probe_clk`), NOT
  counted in the 100, proving the harness reports blocked encodings
  blocked with the exact missing data named.

## Loop results (exact counts, 100 designs, 1000 records)

| block | green | red | blocked | diff_bits_sum |
|---|---|---|---|---|
| LUT_INIT | 0 | 50 | 50 | 25,848 |
| FF_INIT | 1 | 47 | 52 | 834 |
| BRAM_CONTENT | 0 | 0 | 100 | 0 |
| BRAM_PARITY | 0 | 0 | 100 | 0 |
| INT_PIP | 50 | 0 | 50 | 0 |
| IO_CONFIG | 0 | 0 | 100 | 0 |
| CLK | 0 | 0 | 100 | 0 |
| DSP48E2 | 0 | 0 | 100 | 0 |
| URAM288 | 0 | 0 | 100 | 0 |
| FRAMING | 45 | 5 | 50 | 5 |

Per batch, runtime seconds (10 designs each): batch0 516.0, batch1
987.7, batch20 365.9, batch30 510.8, batch40 497.0, batch50 262.4,
batch60 654.4, batch70 214.7, batch80 202.7, batch90 217.7.
Total 4,429.3s (73.8 min). Emitter: release `helion-bitgen` built
from this branch (release build 4m 08s). Core designs take 40-240s
wall each (release parse about 10s CPU per call, throttled by machine
load); BRAM designs take about 1s (emit fails fast). Batch manifests
and JSONL live in the crew harness `runs/` dirs; the aggregate is in
`~/workspace/ultra-bitgen-crew/logs/us07-loop-aggregate.json` and the
human-readable summary in `us07-loop-summary.log`.

## Findings (each measured, with the evidence in the logs)

- F1, LUT/FF Y offset (largest diff, greedy queue head): the emitter
  resolves LUT INIT through the segbits DB only, whose per-bit lines
  carry the Y_ofst=0 mapping; it never consults the BitFiltrator
  LutLoc/RegLoc Y_ofst tables in `bitloc.rs`. Verified on
  SLICE_X48Y80 A6LUT: emitted set bits equal the reference at
  y_ofst=0 exactly (25/25) and the reference at the true y_ofst=20
  not at all (0/25). FF minors are constant across Y but frame
  offsets vary, so FF_INIT is red the same way for any INIT=0/SRVAL=0
  FF off Y0. The goldens only ever exercised SLICE_X0Y0, so this
  never showed. Fix belongs to the bitgen-core lane (consume the
  Y_ofst tables in emit); it is a new sub-problem, not US-07.
- F2, BRAM emit absent on this base: all 50 BRAM designs fail emit
  with `tile RAMB18_X7Y85: unknown feature INIT[16383:0]`. The BRAM
  emit code lives only on `fm/ultra-bitgen-bramgolden` (2e94fc39,
  1b02834a, fdd4e1f, critic PASS); base 07d0d7d does not contain it.
- F3, INT tile Y convention mismatch: the harness reference reads
  INT Y as an absolute row (row major = Y // 60) while the emitter
  reads Y as the FAR row (row major = Y); they agree only at Y=0.
  The xczu3eg tilegrid evidence (INT_X11Y60) supports the harness
  reading. Generated designs use INT_X<col>Y0 only, columns varying
  over all 113. Fixing emit semantics is bitgen-core work.
- F4, zero-bit tie PIPs vs the round-trip contract: the 5 FRAMING reds
  (rand057, rand073, rand079, rand085, rand097) all contain a
  VCC_WIRE PIP whose segbits entry is must-be-zero bits only, so the
  feature sets zero bits and parse-back cannot render it. INT_PIP
  stays green on those designs. This is a harness-contract edge
  (parse(emit(f))==f cannot hold for a zero-bit feature), for the
  diff-harness/bitgen-core lanes.
- F5, blocked blocks stay blocked with named reasons: IO_CONFIG on
  the missing xcku3p tilegrid; DSP48E2/URAM288/CLK not exercised by
  the loop designs, and the probes prove their records name the exact
  missing encoding (DSP: site pins only, needs Vivado fuzzing; URAM:
  absent from both public sources; CLK: no RCLK/CMT tile mapping in
  the emitter).
- F6, vacuous green: rand047 FF_INIT green is vacuous (both FFs
  INIT=1/SRVAL=1, all 4 reference bits 0, emitter sets nothing).
  rand031 and rand061 drew zero FFs, so FF_INIT is blocked as not
  exercised for them.

## Commits on fm/ultra-bitgen-us07

- 2b677a05: generator plus 100-design loop (generator, driver, tests,
  100 designs, 3 probes).
- d85d1233: INT tiles restricted to Y0 (F3) plus regenerated designs.
- (this report, next commit).

No merges were made. Nothing was pushed.
