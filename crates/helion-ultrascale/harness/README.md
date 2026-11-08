# UltraScale+ bitgen diff harness

The scoreboard for the block-by-block greedy loop: for each golden
design on each part, it says per block how far Helion's bitstream is
from identical to the tier-1 reference, largest diff first. That
ordering IS the crew's work queue.

## Run it

```
python3 diff.py
```

One command runs the full diff on blinky and counter for xcku3p and
xcvu3p, prints the four greedy queues, and writes JSONL records plus
a run manifest to `runs/<UTC timestamp>/`. Every record carries the
run seed and the placement hash. A run whose placement hash differs
from the golden meta is rejected (exit 3), not compared.

Options: `--design blinky|counter|all`, `--part xcku3p|xcvu3p|all`,
`--reference-bit PATH` (tier-2 slot), `--emitter-cmd CMD`,
`--out DIR`. Tests: `python3 -m unittest discover -s tests`.
Hand verification: `python3 tests/hand_verify_lut.py`.

## What identical means here

- Tier 1 (today, no Vivado): every bit Helion emits equals the bit
  implied by the public segbits databases (prjuray-db, CC0) and
  BitFiltrator arch tables (MIT) for the same FASM design.
- Tier 2 (needs Vivado): the same design built by Vivado, bit for
  bit. Blocked on the license server since Oct 22, 2026.
  `--reference-bit` records a Vivado bitstream for the tier-2 slot;
  with no reference bit present the run reports tier 1 only and says
  so in the manifest. The harness never invents a reference.

## Records

One JSONL record per block per design per part:

```json
{"block": "LUT_INIT", "design": "blinky", "part": "xcku3p",
 "diff_bits": 0, "total_bits": 512, "status": "green"}
```

Status is `green` (diff 0), `red` (diff > 0), or `blocked` (missing
data, with a `missing` field naming the exact data). Blocked records
carry `diff_bits` null, never zero: a comparison that did not run
must not look like a pass. A harness that reports green on a
comparison it did not run is a FAIL.

`total_bits` is the number of reference-constrained bit positions in
the block. `diff_bits` counts positions where emitted and reference
values differ, plus emitted set bits the reference does not constrain.
`coord_system` says how the reference bits are addressed: `absolute`
(full FARs), `absolute-provisional` (INT PIPs: INT column major =
CLB column major - 1, derived from the xczu3eg tilegrid, needs
xcku3p tilegrid confirmation), or `tile-relative` (IO/CLK: absolute
FARs need the target part's tilegrid, which no public DB ships).

## Greedy queue

Blocks ordered by effective size descending: `diff_bits` when the
comparison ran, else `total_bits` (the size of the work still
standing). Ties: red before blocked before green, then block name.
Fix the largest first, one block per fix, one fix per commit, re-run
the full diff after each fix. A fix that regresses a green block is
reverted on the spot. A block goes green only on zero diff plus a
critic re-run.

## Current state (first run, 2026-11-07)

All 40 records blocked. No emitter exists yet (bitgen-core), so every
block that needs one reports blocked with the missing piece named.
Reference totals on the control part xcku3p: blinky LUT_INIT 64,
FF_INIT 1, INT_PIP 11, IO_CONFIG 55; counter LUT_INIT 256, FF_INIT 4,
INT_PIP 17, IO_CONFIG 55. On xcvu3p, LUT/FF/INT resolve to nothing:
the xcvu3p device file does not exist yet, so those blocks report
blocked naming the device-file worker. IO resolves tile-relatively
on both parts. BRAM/DSP/URAM are not exercised by the golden designs.

## Per-block reference status

| Block | Reference source | Status |
|---|---|---|
| LUT_INIT | BitFiltrator LutLoc, cross-checked vs prjuray CLEL/CLEM INIT lines (all 8 LUTs, 64 bits each, zero mismatches every run) | Resolves, absolute |
| FF_INIT | BitFiltrator RegLoc, cross-checked vs prjuray INIT.V0/V1 (all 16 FFs); polarity inverted per XAPP1230 (frame bit = 1 - INIT) | Resolves, absolute |
| BRAM_CONTENT / BRAM_PARITY | BitFiltrator BramMemLoc/BramMemParityLoc, cross-checked vs prjuray segbits_bram.block_ram.db (INIT_00[0..4] matches exactly) | Resolves; not exercised by goldens |
| INT_PIP | prjuray segbits_int.db (3778 PIPs); absolute FARs via INT col = CLB col - 1 (provisional) | Resolves, absolute-provisional |
| IO_CONFIG | prjuray segbits_hpio_right.db (1560 lines) | Tile-relative only; absolute needs xcku3p tilegrid |
| CLK | prjuray segbits_cmt_right.db has CLK_HROUTE PIPs only; no BUFGCE/MMCM config encoding anywhere public | Blocked: needs Vivado fuzzing |
| DSP48E2 | prjuray site_type_DSP48E2.json has pins only; BitFiltrator has no DSP data | Blocked: needs Vivado fuzzing |
| URAM288 | No encoding in either public source | Blocked: needs Vivado fuzzing or device file |
| FRAMING | Round-trip parse(emit(f)) == f plus sync-word and IDCODE byte checks | Blocked: needs emitter |

## Golden designs

`golden/blinky.<part>.fasm` and `golden/counter.<part>.fasm`, generated
by `gen_golden.py` from `examples/blinky.sv` and `examples/counter.sv`.
All logic is placed in SLICE_X0Y0. LUT INIT values are computed from
the boolean functions, never hand-typed. Seed 20261107 is recorded in
every meta file and every record. Placement hash (sha256 over the
placement record plus the canonical FASM) is recorded in the meta and
recomputed on every run.

INT and HPIO tile coordinates in the goldens are PROVISIONAL: the INT
tile X is the logical CLB column index, not a verified Vivado tile X,
because no xcku3p tilegrid exists in any public DB. The PIP and IO
feature strings are verified to exist in their segbits DBs
(`gen_golden.py` fails loud otherwise). The route is the frozen chosen
target, not a Vivado-produced route; routability validation awaits the
router worker plus the xcku3p tilegrid.

xcvu3p goldens reuse the xcku3p placement verbatim and are flagged
provisional in their meta until the xcvu3p device file lands.

## Emitter protocol

The harness discovers an emitter from `--emitter-cmd` or the
`HELION_BITGEN_EMITTER` environment variable, and nothing else. The
emitter implements `emit`, `parse`, and optionally `bits` (see
`emitter.py` for the exact protocol; `bits` prints block-labelled
lines `FAR_HEX OFFSET VALUE BLOCK`). With no emitter, blocks report
blocked. The `bits` path compares against the real .bit bytes; without
it the harness derives the emitted bitset through `parse` and records
`emitted_via: parse-mediated` (weaker: it cannot catch a bug shared
by emit and parse). A fully independent .bit byte decoder (UG570
packet decoding) is a v2 harness item; until then FRAMING is checked
by round-trip plus sync-word/IDCODE byte checks.

## Layout

- `diff.py`: CLI entry, runs all combos, prints queues, writes JSONL.
- `fasm.py`: FASM parser, canonical serializer, block classifier.
- `reference.py`: tier-1 reference (BitFiltrator + prjuray), resolvers,
  cross-checks. `MissingReference` names the exact gap.
- `emitter.py`: emitter discovery and protocol.
- `roundtrip.py`: round-trip + independent byte checks (FRAMING).
- `diff_engine.py`: per-block diff, greedy ordering, JSONL writing.
- `gen_golden.py`: generates the frozen goldens and placement hashes.
- `golden/`: frozen FASM + meta (seed, placement hash).
- `reference/`: vendored reference data (see SOURCES.md).
- `tests/`: unit tests (stdlib unittest) plus `hand_verify_lut.py`
  and the test-only synthetic emitter `fake_emitter.py`.
- `runs/`: run outputs (JSONL + manifest per run).

## Standing rules

No em dashes anywhere. Never fabricate: unknown encodings fail loud,
unrun comparisons report blocked, and every claim in a report ships
with the exact command and its output.
