# US-07 loop rerun on the F1-F4 branch, report

Branch: `fm/ultra-bitgen-core-f1f4`, tip `7c332625`.
Status: loop complete, 100/100 designs diffed at tier 1 on xcku3p.
Zero red records. Nothing here claims Vivado equivalence; tier 2
waits for Vivado, license state UNKNOWN.

## What changed since the baseline

The baseline loop ran on base `07d0d7d` (branch
`fm/ultra-bitgen-us07`, report `US07_LOOP_REPORT.md`). This rerun
uses the same generator seed (20261119) and the same 100 designs,
byte-identical (verified by `diff -r` against the US-07 worktree),
against the fixed emitter on this branch:

- F1: LUT/FF INIT resolves through the BitFiltrator Y_ofst tables
  off Y0 (commit `d47e5917`).
- F2: BRAM emit verified present; all 50 BRAM designs emit (no code
  change).
- F3: INT tile Y is absolute, FAR row = Y // 60 (commit `770ab73d`);
  parse-back renders INT features only on row-start tiles
  (Y % 60 == 0).
- F4: zero-bit PIP round-trip contract (commit `4194cda3`);
  unobservable features excluded from the FASM comparison and
  verified at the bit level.
- FAR increment: geometry-aware in the parse path (commit `cc977d71`).
- `bits` subcommand: LUT/FF table branch (commit `7c332625`, the tip).

## Loop results (exact counts, 100 designs, 1200 records)

The loop now records 12 blocks per design, not 10: LUTRAM and
CARRY8 joined the harness on this branch after the baseline. The
table below shows the 10 baseline blocks before and after, then the
2 new blocks.

| block | baseline green/red/blocked | rerun green/red/blocked | baseline diff bits | rerun diff bits |
|---|---|---|---|---|
| LUT_INIT | 0 / 50 / 50 | 100 / 0 / 0 | 25,848 | 0 |
| FF_INIT | 1 / 47 / 52 | 96 / 0 / 4 | 834 | 0 |
| BRAM_CONTENT | 0 / 0 / 100 | 0 / 0 / 100 | 0 | 0 |
| BRAM_PARITY | 0 / 0 / 100 | 0 / 0 / 100 | 0 | 0 |
| INT_PIP | 50 / 0 / 50 | 100 / 0 / 0 | 0 | 0 |
| IO_CONFIG | 0 / 0 / 100 | 0 / 0 / 100 | 0 | 0 |
| CLK | 0 / 0 / 100 | 0 / 0 / 100 | 0 | 0 |
| DSP48E2 | 0 / 0 / 100 | 0 / 0 / 100 | 0 | 0 |
| URAM288 | 0 / 0 / 100 | 0 / 0 / 100 | 0 | 0 |
| FRAMING | 45 / 5 / 50 | 100 / 0 / 0 | 5 | 0 |

New blocks (no baseline row; the baseline harness did not record
them):

| block | rerun green/red/blocked | rerun diff bits |
|---|---|---|
| LUTRAM | 0 / 0 / 100 | 0 |
| CARRY8 | 100 / 0 / 0 | 0 |

Per batch, runtime seconds (10 designs each): batch0 317.7, batch1
317.5, batch20 315.1, batch30 317.1, batch40 319.0, batch50 319.6,
batch60 315.0, batch70 315.3, batch80 314.9, batch90 316.2.
Total 3,167.4s (52.8 min), faster than the baseline 4,429.3s
(73.8 min): every design now takes about 31.7s wall, where the
baseline BRAM designs failed emit in about 1s and core designs took
40-240s. Emitter: release `helion-bitgen` rebuilt from this tip
(forced rebuild, 29.20s; binary 5,434,904 bytes, Nov 20 10:27 UTC).
Placement-hash verification passed for all 100 designs (0
REJECTED lines in the chain log; all 10 batches exit 0). Batch
manifests and JSONL live in the crew harness
`runs/us07-rerun-f1f4-batch*/` dirs; the aggregate is in
`~/workspace/ultra-bitgen-crew/logs/us07-rerun-aggregate.json` and
the human-readable summary in `us07-rerun-summary.log`.

Reference bit totals compared (record `total_bits` sums, identical
to the baseline on every exercised block): LUT_INIT 50,944, FF_INIT
1,466, INT_PIP 1,052, IO_CONFIG 5,500, FRAMING 300 checks (100
designs x 3, up from 150: the 50 BRAM designs now round-trip).

## Findings (each measured, with the evidence in the logs)

- F1 fixed, LUT_INIT: 100 green, 0 diff bits, over 50,944 compared
  bits. The baseline 25,848 diff bits are gone.
- F1 fixed, FF_INIT: 96 green, 0 diff bits, over 1,466 compared
  bits. The baseline 834 diff bits are gone. The 4 blocked records
  are rand031, rand038, rand056, rand061, which drew zero FFs (their
  FASM files carry no FF feature line, only the generator header
  comment); the block is not exercised for them. The baseline named
  only rand031 and rand061 because the other two are BRAM designs
  whose FF count was masked by the emit failure. rand047 stays green
  in the same shape as baseline finding F6: total 4, diff 0, all 4
  reference bits 0 (both FFs INIT=1/SRVAL=1), so it proves the
  emitter sets nothing it should not, not that it can set an FF bit;
  the other 95 green FF records carry the weight.
- F2 confirmed, BRAM emit present: all 50 BRAM designs emit and
  round-trip. Direct check, outside the loop, on rand000 (RAMB18_X7Y85):
  `helion-bitgen emit` wrote a 16,585,732-byte `.bit`, and `parse`
  returned the INIT line (4,096 hex chars) and the INITP line (512
  hex chars) exactly equal to the golden values. FRAMING is green on
  all 50 BRAM designs, which it could not be if emit dropped the
  BRAM lines. BRAM_CONTENT and BRAM_PARITY themselves stay blocked,
  see below: the bits are emitted but the harness has no reference
  resolver to compare them against.
- F3 fixed, INT_PIP: 100 green, 0 diff bits. The baseline 50 blocked
  were the BRAM designs (emit failure); the core 50 were already
  green.
- F4 fixed, FRAMING: 100 green. The 5 baseline reds (rand057,
  rand073, rand079, rand085, rand097) are green with exactly 1
  skipped check each, each naming its unobservable VCC_WIRE PIP:
  rand057 `INT_X87Y0.PIP.IMUX_W23.VCC_WIRE`, rand073
  `INT_X86Y0.PIP.IMUX_E23.VCC_WIRE`, rand079
  `INT_X62Y0.PIP.IMUX_E16.VCC_WIRE`, rand085
  `INT_X45Y0.PIP.IMUX_E24.VCC_WIRE`, rand097
  `INT_X98Y0.PIP.BYPASS_E10.VCC_WIRE`. Their INT_PIP records stay
  green: the zero bits are verified at the bit level, per the
  contract.
- CARRY8 green is unexercised green, not a CARRY8 proof: every
  record has total_bits 0, emitted via `bits`, because the generator
  writes no CARRY8 features. The real CARRY8 evidence stays the
  separate CARRY8 golden (11 bits, 0 diff).
- Remaining reds: none. Zero red records across 1,200 records.

## Remaining blocked records, with the named gap each

- BRAM_CONTENT x100, BRAM_PARITY x100: the harness has no BRAM
  reference resolver. `diff_engine.py` raises `MissingReference`
  from its hardcoded `NOT_EXERCISED` table before looking at the
  design, so the record's missing string ("no BRAM instances in the
  golden designs") is stale wording: the generated BRAM designs do
  carry RAMB18 INIT/INITP lines, and the emitter emits them (see F2
  above). The gap is a missing reference encoding for RAMB18 INIT in
  the harness, not an emitter defect. Unblocker: a BRAM reference
  resolver in the harness (prjuray segbits for RAMB18 INIT, or the
  BRAM golden's bit mapping), a harness-lane job.
- IO_CONFIG x100: reference bits are tile-relative; absolute FARs
  need the xcku3p tilegrid (prjuray-db ships xczu3eg tilegrids
  only). Same gap as the baseline. Unblocker: a public xcku3p
  tilegrid.
- DSP48E2 x100: no DSP48E2 instances in the golden designs; the
  generator writes no DSP features, and the encoding is absent
  (site pins only, needs Vivado fuzzing). Same gap as the baseline.
- URAM288 x100: no URAM288 instances in the golden designs; the
  generator writes no URAM features, and the encoding is absent from
  both public sources. Same gap as the baseline.
- CLK x100: the generator writes no CLK features, so the block is
  not exercised by any of the 100 designs. Same gap as the baseline.
- LUTRAM x100: the generator writes no LUTRAM features, so the block
  is not exercised by any of the 100 designs. New block, same class
  of gap: the loop needs LUTRAM-bearing generated designs before
  this block can go green or red here.
- FF_INIT x4: rand031, rand038, rand056, rand061 drew zero FFs (see
  above). Generator-draw gap, not an emitter gap.

## Setup and protocol notes

- `gen_random.py`, `run_100.py`, and `generated/` were copied from
  the US-07 worktree (`fm/ultra-bitgen-us07`, tip `d2e080b7`) into
  this worktree's harness and verified byte-identical (`diff -r`,
  206 files). Designs were not regenerated; seed base is 20261119.
- `gen_golden.py`: ported exactly the `ff_locations` change from
  the US-07 branch (`design_ff_locations` honors a per-design
  `ff_locations` list so multi-slice designs complete FF defaults
  on their real tiles). All other harness diffs between the two
  worktrees are this branch's newer work (LUTRAM, CARRY8, RCLK/CMT,
  CLK, F4 contract, emitter timeout 300s to 1800s) and were kept.
  After the port, the remaining `gen_golden.py` diff to the US-07
  branch is only this branch's uram/lutram golden additions.
- `harness/reference/` in this worktree is the untracked local copy
  populated by Worker D; verified byte-identical to the live crew
  harness reference (`diff -r`, no differences). It is not committed.
- Harness suite after the port: 84 tests, OK
  (`python3 -m unittest discover -s tests`, 24.5s).
- Build: the pre-existing release binary predated the tip commit
  object (built 10:11 UTC from the same file contents, commit
  recorded 10:23 UTC), so the sources were touched and the binary
  was rebuilt from tip `7c332625` (29.20s, log
  `~/workspace/ultra-bitgen-crew/logs/rerun-build.log`).
- Batches ran strictly sequentially through
  `~/workspace/ultra-bitgen-crew/logs/us07-rerun-chain.sh`
  (deviation: the original overlapped batch0 and batch1; sequential
  was chosen because the box OOMs under parallel load). `--log-dir`
  was passed explicitly to put per-batch logs in the crew logs dir,
  the same destination the original run's default produced.
- A one-design smoke run of rand000 preceded the loop (excluded
  from the aggregate). Its first attempt used a wrong emitter path
  (`crates/.../harness/../../target/release/helion-bitgen`, which
  does not exist) and reported all 12 records blocked with
  "emitter command not found"; it was discarded and rerun with the
  correct path (33.1s, green 5, red 0, blocked 7). No batch needed a
  retry; no batch failed.
- No Rust source was edited. No merges, no pushes.

## Commits on fm/ultra-bitgen-core-f1f4

- (this report, plus the copied loop driver and the ported
  `ff_locations` change, next commit).

No merges were made. Nothing was pushed.
