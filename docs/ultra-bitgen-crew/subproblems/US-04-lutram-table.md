# US-04: LUTRAM table

- Track: UltraScale+, bitgen.
- Branch: fm/ultra-bitgen-lutram (worktree
  ~/workspace/helion-ultra-bitgen-lutram, cut from fm/ultra-bitstream
  07d0d7d). No merges.
- Status: DERIVED (tier 1). The LUTRAM content-bit table IS the LUT
  INIT table (`LutLoc`); the mode-selection bits live in the vendored
  prjuray segbits DBs. A LUTRAM golden diffs to zero in the harness on
  xcku3p. Critic re-run still needed before the block goes green.
  Tier 2 (a Vivado bitstream diff) remains the arbiter for the
  mode-line SELECTION only; the bit LOCATIONS are proven below.

## Goal

Encode LUT RAM so designs that use distributed RAM can diff to zero.

## Done when

LUTRAM reads green on a golden design that uses LUT RAM, or it is proven
with evidence that only Vivado logic-location data can supply the table,
with the exact missing data named.

## What is done (branch fm/ultra-bitgen-lutram, commits 87729bd, d8c3771)

- `crates/helion-ultrascale/src/bitloc.rs`: `lut_placed_bits` with
  `LutMode::Lutram` returns the same 64 locations as logic mode on
  SLICEM tiles (CLEM/CLEM_R), labeled block `LUT_INIT`; a SLICEL
  request fails loud with `LutramRequiresSlicem`. New `lutram_fasm`
  (RAM64X1S lines: `INIT[63:0]` plus `xLUT.MODE.RAM`,
  `xLUT.DI1.DI1`, `xLUT.SIZE.LARGE`) and `lutram_slice_fasm`
  (slice-wide `WA7/8/9USED.V0`); the H LUT fails loud with
  `LutramNoDataInput` (no DI1 feature in the vendored DB). The old
  `LutramBlocked` error is gone.
- `crates/helion-ultrascale/tests/lutram.rs`: 8 tests re-checking the
  derivation on every run (table identity, segbits agreement, exact
  mode BitRefs, no INIT collision, SLICEL and H-LUT fail-loud).
- Harness: new `LUTRAM` block (mode bits only; content bits classify
  as `LUT_INIT`), `reference.resolve_lutram_mode`, and a `lutram`
  golden (one RAM64X1S on SLICE_X1Y0 ALUT, INIT
  0x6996966996696996). `TestLutram` (8 tests) pins the golden, the
  exact mode bits, and a zero-diff LUTRAM/LUT_INIT run with the fake
  emitter on xcku3p; xcvu3p reports blocked with the missing data
  named. Full-run record count is 66 (6 combos x 11 blocks).

## Derivation: why the content bits are the LUT INIT bits

Three independent public sources agree. None of this rests on the
paper section V-B: the earlier V-B claim (that the paper states
LUTRAM shares LUT INIT bits) stays REJECTED. The derivation rests on
the BitFiltrator extraction code, which says it directly.

1. BitFiltrator's own extraction code,
   `bitfiltrator-ref/src/extract_encoding_from_logic_loc.py`
   (the `LutramLoc`/`LutLoc` branch, lines 513-517): "the INIT bits
   we were interested in ended up having the same encoding for
   LUTRAMs and standard LUTs when we extracted the data", so both
   map to the one `LutLoc` key via `get_LutramLoc_encoding`, which
   asserts all 64 bits (0-63) present per LUT per Y offset. Its
   pipeline (`create_arch_summary.py`, the SLICEM loop) extracts
   CLEM/CLEM_R `LutLoc` from Vivado logic-location files of
   all-LUTRAM columns (`^LutramLoc$`), while the SLICEL loop
   extracts CLEL_L/CLEL_R `LutLoc` from a LUT INIT fuzzing sweep.
   The shipped CLEM `LutLoc` table IS the LUTRAM-extracted table.
2. The vendored tables are byte-identical across all four CLB tile
   types: 0 differing pairs out of 30,720 per tile-type pair (60 Y
   rows x 8 LUTs x 64 bits), checked by `tests/lutram.rs` and
   independently by a Python script over
   `data/ultrascale_plus_lut_ff_bram.json`. The LUTRAM-extracted
   table and the LUT-sweep table are the same table.
3. prjuray-db segbits agree bit for bit: every `xLUT.INIT[i]` line
   in all four vendored `segbits_cle*.db` files equals the `LutLoc`
   Y0 pair (512 of 512 per tile type), checked by `tests/lutram.rs`
   and independently by a Python script.

Mode bits, read from the vendored `segbits_clem.db` lines (not from
code): `ALUT.MODE.RAM` sets (9, 19) and clears (11, 18); `MODE.LUT`
and `MODE.SRL` are the other two combinations of the same two bits,
so exactly one mode holds at a time. `ALUT.DI1.DI1` sets (10, 19)
(`DI1.SIN` clears it); `ALUT.SIZE.LARGE` clears (8, 18)
(`SIZE.SMALL` sets it); `WA7USED.V0` clears (8, 23),
`WA8USED.V0` clears (11, 22), `WA9USED.V0` clears (9, 22). No mode
bit collides with any INIT bit. SLICEL DBs carry no `MODE.RAM`
lines at all (grep count 0). The H LUT has no DI1 feature
(grep count 0), so its data-input path is unverified and fails loud.

## What is NOT proven (tier 2 caveat)

The mode-line SELECTION for a 64x1 single-port RAM (RAM64X1S)
follows the f4pga Project X-Ray DRAM documentation (7-series
feature family, same concepts under UltraScale+ spellings): 64x1S
needs `xLUT.RAM` plus the data-in mux to the LUT's own input
(`DI1.DI1`), `SMALL` only for 32-bit use, `WA7/8USED` only for
128x1/256x1 use. That selection is a documented cross-family
inference, not a Vivado-verified UltraScale+ fact: tier 2 (a
Vivado bitstream diff) remains its arbiter. The bit LOCATIONS are
proven by the three sources above; only the SELECTION of the mode
values awaits tier 2. This caveat is recorded in the code docs, the
golden header, and `lutram_fasm`.

Related open items, unchanged: the `SIZE.SMALL` label is an
approximation (a logic 5-LUT use of the same bit would mislabel;
recorded in `fasm.py`, split later if a design ever sets it outside
a RAM/SRL). The Rust `64'h...` INIT spelling vs the golden `0x...`
spelling match after roundtrip normalization, not as raw strings
(`roundtrip.py`; pinned by `TestLutram`).

## What was tried and what failed

- Paper section V-B "same bits" claim: REJECTED (CTO ruling
  2026-11-07, recorded in INTERFACES.md). The paper describes
  separate extraction methods and never states LUTRAM shares the
  LUT INIT bits. Do not revive it; the derivation above replaces
  it with the extraction code as the primary source.

## What to do next

1. Critic re-run of the LUTRAM branch before the block goes green.
2. Tier 2: a Vivado bitstream diff of a RAM64X1S design, the day
   the license works, to confirm the mode-line selection.
3. INTERFACES.md block registry: LUTRAM is still listed there as
   BLOCKED with `LutramBlocked`; the orchestrator updates it to the
   derived status on merge of this branch (workers do not edit the
   crew copy; the crew harness copy at
   ~/workspace/ultra-bitgen-crew/harness/ also still needs these
   LUTRAM changes applied at integration time, plus it carries BRAM
   goldens this branch does not have).
4. US-07 (random generator): LUTRAM coverage is unblocked now.

## Files and commands

- Contracts: docs/ultra-bitgen-crew/INTERFACES.md (block registry).
- Code: crates/helion-ultrascale/src/bitloc.rs,
  crates/helion-ultrascale/tests/lutram.rs.
- Harness: crates/helion-ultrascale/harness/fasm.py (classify),
  reference.py (resolve_lutram_mode), gen_golden.py (lutram golden).
- Related: US-01 (same class: missing public data, Vivado as unblocker).

## Dependencies

No longer blocked on public data for the content bits or the mode
bits. Tier 2 confirmation waits on the Vivado license (see US-08).
Unblocks LUTRAM coverage in US-07 (the random generator).

## Rules

- No em dashes anywhere.
- Never invent bits. Fail loud on missing encodings.
- "Probably unfillable" is not done; name the exact missing data.
