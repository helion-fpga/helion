# CARRY8 golden design

An 8-bit incrementer on one CARRY8, built from public sources and
existing Helion code only. No Vivado, no cluster, no license. It
moves CARRY8 from unexercised (0/0 bits) to exercised: the harness
checks 11 CARRY8 bit positions on xcku3p at tier 1.

## Files

- `examples/carry8.sv`: the design (behavioral, as in `counter.sv`).
- `crates/helion-ultrascale/harness/golden/carry8.xcku3p.fasm` and
  `carry8.xcku3p.meta.json`: the frozen golden and placement hash.
- `crates/helion-ultrascale/harness/golden/carry8.xcvu3p.fasm` and
  `carry8.xcvu3p.meta.json`: same placement, provisional (no xcvu3p
  device file yet, so every block reports blocked there, as for the
  other goldens).
- `crates/helion-ultrascale/harness/gen_golden.py`: the generator
  (the FASM is computed, never hand-typed; `gen_golden.py` fails loud
  if a CARRY8 feature is absent from `segbits_clel_r.db`).
- `crates/helion-ultrascale/harness/diff.py`: `carry8` is a design
  choice (`--design carry8`).
- `crates/helion-ultrascale/harness/tests/test_harness.py`:
  `TestCarry8Golden` pins the 11-bit reference bitset, the meta
  hashes, the green diff with the fake emitter, the red flip case,
  and the absence of any CI_TOP line.

Regenerate: `python3 gen_golden.py` from the harness dir (needs the
vendored `reference/` data, as for the other goldens). The blinky
and counter placement hashes are unchanged by the generator edit.

## The design

`cnt <= cnt + 1` on every clock, `led = cnt[7]`, all in SLICE_X0Y0
(xcku3p: row major 0, CLB column 0, tile type CLEL_R). One slice
holds exactly one 8-bit incrementer: 8 LUTs, 8 FFs, 1 CARRY8.

The CARRY8 mapping, per UG574 Figure 2-4 and UG974:

- Each CARRY8 stage is a MUXCY plus an XOR: `CO[i] = S[i] ? CI[i] :
  DI[i]` (carry out) and `O[i] = S[i] ^ CI[i]` (sum out). The LUT
  O6 output drives S[i] (propagate) and the LUT O5 output drives
  DI[i] (generate); the DI mux selects the LUT output
  (`DIi.<letter>`) or the slice DI pin (`DIi.DIi`).
- Incrementer: set `S[i] = cnt[i]` and `DI[i] = cnt[i]`. Then
  `CO[i] = cnt[i] ? carry[i] : cnt[i]`, which is `cnt[i] AND
  carry[i]` (when `cnt[i]` is 0 the carry dies at 0; when it is 1
  the carry passes through), and `O[i] = cnt[i] ^ carry[i]`, the
  sum bit. With `CI[0] = 1` the chain adds 1.
- Each LUT is a buffer of its count bit: INIT `0xAAAAAAAAAAAAAAAA`
  (`O6 = I0`, `O5 = I0` on the LUT6_2 lower half), so one LUT
  supplies both S[i] and DI[i]. The INIT is computed by
  `gen_golden.py` from the buffer function, never hand-typed.
- `CARRY_TYPE = SINGLE_CY8`: one 8-bit chain.
- `CI = 1` (`CI.V1`): the +1 carry-in.
- `CI_TOP = 0` (ground). UG974: "Tie to ground if
  CARRY_TYPE=SINGLE_CY8." See the omission note below.
- The 8 FFs capture `O[7:0]` on the clock. The clock route and the
  IO lines are copied from the counter golden (same slice, same
  single clock input); the route is the frozen chosen target, not a
  Vivado-produced route, as for the other goldens.

## CARRY8 features exercised

Ten FASM lines on SLICE_X0Y0, resolving to eleven distinct bit
positions (FAR `0x500 + minor`, SLICE_X0Y0 column major 5).
All values are from prjuray-db `segbits_clel_r.db` (CC0); the
harness resolves them with `resolve_carry8`, absolute FARs.

| FASM feature | DB line | FAR | Offset | Value |
|---|---|---|---|---|
| CARRY8.CARRY_TYPE.SINGLE_CY8 | `!12_45` | 0x50C | 45 | 0 |
| CARRY8.CI.V1 | `10_18 !14_01` | 0x50A | 18 | 1 |
| CARRY8.CI.V1 | `10_18 !14_01` | 0x50E | 1 | 0 |
| CARRY8.DI0.AX | `!12_03` | 0x50C | 3 | 0 |
| CARRY8.DI1.BX | `!12_09` | 0x50C | 9 | 0 |
| CARRY8.DI2.CX | `!12_15` | 0x50C | 15 | 0 |
| CARRY8.DI3.DX | `!12_21` | 0x50C | 21 | 0 |
| CARRY8.DI4.EX | `!15_23` | 0x50F | 23 | 0 |
| CARRY8.DI5.FX | `!12_39` | 0x50C | 39 | 0 |
| CARRY8.DI6.GX | `!12_33` | 0x50C | 33 | 0 |
| CARRY8.DI7.HX | `!15_47` | 0x50F | 47 | 0 |

What the harness should see for `carry8` on xcku3p:

- CARRY8 record: `total_bits` 11, `diff_bits` 0, status green,
  `coord_system` absolute, when the emitter is correct. Exactly
  one of the eleven bits is set (`(0x50A, 18)`); the other ten are
  must-be-zero constraints, so an emitter that sets DUAL_CY4, a
  wrong CI selection, or a DI bit goes red.
- The other blocks on this design: LUT_INIT 512 bits (8 LUTs x 64),
  FF_INIT 16 bits (8 FFs x INIT and SRVAL), INT_PIP 17 bits (same 3
  PIPs as the counter golden), FRAMING green on a correct round
  trip, IO_CONFIG blocked tile-relative (no xcku3p tilegrid, as for
  the other goldens).

## Why CI_TOP has no line

The correct CI_TOP value is 0, but `CI_TOP.V0` (`!11_31 !12_27`)
cannot appear in this golden. The emitter's parse-back renders an
all-clear feature only when it is the exact complement of another
feature of the same tile type (bitgen.rs presence rule); no
feature lists both 11_31 and 12_27 as set bits with no `!` bits
(`CI_TOP.V1` lists `11_31 !12_27`, `CI_TOP.EX` lists `!11_31
12_27`), so `CI_TOP.V0` would be dropped on parse-back and the
FRAMING round trip would fail. The golden therefore leaves CI_TOP
unconfigured. The emitted frames still read 0 on both CI_TOP bits
(FAR 0x50B offset 31, FAR 0x50C offset 27), and the per-block diff
catches an emitter that sets either of them: an emitted set bit
outside the reference bitset counts as a diff. The same
observability rule is why `CI.V0` is not used either (the golden
uses the observable `CI.V1` instead).

## Not exercised

Fourteen of the 24 CARRY8 suffixes are not covered by this design:

- `CARRY_TYPE.DUAL_CY4` (two independent 4-bit chains),
- `CI.AX`, `CI.V0` (other carry-in mux selections),
- `CI_TOP.EX`, `CI_TOP.V1`, `CI_TOP.V0` (upper carry-in mux; V0
  also unobservable, see above),
- `DI0.DI0` through `DI7.DI7` (DI from the slice DI pins instead
  of the LUT outputs).

A second golden (for example a dual 4-bit adder in DUAL_CY4 mode
with DI from the DI pins) would be the follow-up for these. This
golden deliberately stays minimal: one slice, one chain, one
configuration, fully observable through parse-back.

## Limits, stated plainly

- No second-source cross-check: BitFiltrator ships no CARRY8
  table, so the reference is prjuray-db alone (recorded in the
  block registry, US-05).
- Y-independence within a column is the DB's own structure, not
  independently verified. Visible consequence: parse-back renders
  `CARRY8.CI.V1` for all 60 slices in the SLICE_X0Y0 column (the
  frame bit is column-shared in this model), and the complement
  CARRY8 lines for every untouched slice in the device. The
  FRAMING comparison scopes to the design's tiles and the bits diff
  dedupes by position, so the records are unaffected; the same
  column-sharing already applies to the LUT/FF goldens.
- Tier 2 (Vivado bitstream for this design, bit for bit) is still
  blocked on the license server; the tier-1 check here is emitter
  bits versus the public segbits DBs for the same FASM.
- INT and HPIO tile coordinates are PROVISIONAL (logical CLB
  column index, no xcku3p tilegrid), as for the other goldens.

## Verification

All commands ran on branch `fm/ultra-bitgen-carry8-golden` (worktree
`~/workspace/helion-ultra-bitgen-carry8-golden`). The harness ran
from a scratch overlay in `/tmp/carry8verify/harness` (the branch's
harness code as real copies, `reference/` symlinked to the crew
harness vendored data, goldens copied in), because the repo harness
carries no `reference/` data dir. Run outputs (diff.jsonl,
manifest.json) are kept in `~/workspace/carry8-verify-runs/`.

1. Harness unit tests (53 tests, 0 failures):
   `cd /tmp/carry8verify/harness && python3 -m unittest discover -s tests`
   Result: `Ran 53 tests in 105.694s / OK` (48 pre-existing
   including the CARRY8 machinery tests, plus the 5 new
   `TestCarry8Golden` tests).
2. Golden generation: `python3 gen_golden.py` prints
   `carry8 xcku3p: placement_hash 923b204a...` and
   `carry8 xcvu3p: placement_hash 76e13186...`; the blinky and
   counter placement hashes are unchanged from the committed metas.
3. Reference resolution (11 bits, 1 set):
   resolving every CARRY8 line of `carry8.xcku3p.fasm` gives
   `(0x50A,18)=1, (0x50E,1)=0, (0x50C,45)=0, (0x50C,3)=0,
   (0x50C,9)=0, (0x50C,15)=0, (0x50C,21)=0, (0x50C,33)=0,
   (0x50C,39)=0, (0x50F,23)=0, (0x50F,47)=0`.
4. Real-emitter diff, carry8/xcku3p:
   `python3 diff.py --design carry8 --part xcku3p --emitter-cmd
   <target/release/helion-bitgen> --out runs/carry8-real`
   Result: CARRY8 green, diff 0, total 11, absolute, via bits;
   LUT_INIT green 512; FF_INIT green 16; INT_PIP green 17
   (absolute-provisional); FRAMING green 3; IO_CONFIG blocked
   tile-relative (55 bits, no tilegrid); BRAM/CLK/DSP/URAM blocked
   (not exercised). Exit 0.
5. Real-emitter diff, full xcku3p (all 3 designs):
   `python3 diff.py --part xcku3p --emitter-cmd <binary>
   --out ~/workspace/carry8-verify-runs/full-xcku3p-real`
   Result: 33 records. Blinky and counter unchanged (CARRY8
   zero-bit green, LUT 64/256, FF 2/8, INT 11/17, FRAMING 3
   green); carry8 as in (4). Exit 0. One earlier attempt at this
   run died with `OSError: [Errno 28] No space left on device`
   because /tmp is a 512M tmpfs and each emitted .bit is
   16,585,732 bytes; the truncated carry8 roundtrip.bit
   (9,867,264 bytes) made FRAMING report blocked in that attempt.
   Re-running with `--out` on the home filesystem gave FRAMING
   green, confirming the blocked record was the disk-full
   artifact, not a design failure.
6. Emitted bits, direct check:
   `helion-bitgen bits --bit <carry8 emit.bit> --part xcku3p`
   prints exactly one CARRY8-labelled set bit:
   `0000050a 18 1 CARRY8` (the CI.V1 set bit; all other CARRY8
   positions read 0 as required).
7. Parse-back, direct check:
   `helion-bitgen parse --bit <carry8 emit.bit> --part xcku3p`
   renders exactly the 10 golden CARRY8 lines for SLICE_X0Y0 and
   zero `CI_TOP` lines anywhere in the device (7 MB parse-back,
   244,140 CARRY8 lines total, all complement lines on untouched
   slices). `CI.V1` renders for all 60 slices SLICE_X0Y0..Y59:
   the frame bit is column-shared in this model; the FRAMING
   comparison scopes to the design's tiles and the bits diff
   dedupes by position, so the records are unaffected.
8. Rust binary tests: `cargo test --release -p helion-ultrascale
   --bin helion-bitgen` (see the handoff report for the outcome;
   the binary code is byte-identical to the critic-verified
   `fm/ultra-bitgen-carry8` branch, verified by empty
   `git diff fm/ultra-bitgen-carry8 HEAD` on the cherry-picked
   files).
