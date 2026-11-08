# Verification log

All commands run from `~/workspace/ultra-bitgen-crew/harness` on
2026-11-07 (UTC). Python 3.12, stdlib only. Seed 20261107
throughout.

## Unit tests

Command: `python3 -m unittest discover -s tests`

Result: `Ran 20 tests ... OK` (was 18; +2 for the FLAG 2 fix, see
below). Coverage: FASM parse/serialize
round-trip on all 4 goldens; parser rejects garbage; classifier
counts per golden (blinky: LUT 1, FF 1, INT 2, IO 2; counter: LUT 4,
FF 4, INT 3, IO 2); INIT values recomputed from truth tables
independently; blinky A6LUT spot values (bit 0 at FAR 0x50B offset 15
= 1, bit 1 at FAR 0x50A offset 15 = 0); FF inverted polarity; cross-
checks for all 8 LUTs and 16 FFs at Y offset 0; INT PIP resolves
(BOUNCE PIP 6 bits, FAR 0x4400 range); unknown PIP raises
MissingReference; xcvu3p device file raises MissingReference;
DSP/URAM report missing containing "absent"; compute_diff counts both
directions (flipped required bit + extra set bit = 2); blocked without
emitter (10 records, all blocked, diff_bits null, missing present;
blinky/xcku3p totals LUT 64, FF 1, INT 11, IO 55); greedy order
largest first; fake-emitter green (LUT/FF/INT/FRAMING green, IO stays
blocked tile-relative) and red (3 flipped LUT bits gives LUT_INIT red
diff_bits 3, FF stays green; corrupt parse gives FRAMING red); CLI
full run (40 records, all blocked, exit 0, manifest tier1, 4 combos);
tampered golden rejected (exit 3, REJECTED); missing --reference-bit
exits 2.

Two bugs were found and fixed while the tests were written: the fake
emitter imported `reference` from the tests dir instead of the harness
dir, and the per-block diff compared the whole-design emitted bitset
against each block (other blocks' set bits counted as diffs). The
fix: the `bits` protocol carries block labels
(`FAR OFFSET VALUE BLOCK`), and `diff_design` compares per block.
The fake emitter also now supports `FAKE_EXTRA_BIT_BLOCK`: an emitter
bit in a block the design does not use reports red, not green.

## FLAG 2 fix (critic session 5): garbage emitter parse output

Reproduced first, exactly as the critic reported it. A shell emitter
whose emit writes a dummy .bit and whose parse prints "this is not
fasm at all", run as `python3 diff.py --design blinky --part xcku3p
--emitter-cmd /tmp/critic5_badparse.sh --out /tmp/critic5_badparse_run2`:
exit 1, traceback ending `fasm.FasmError: emitter parse output:1: no
tile.feature separator in line 'this is not fasm at all'`, from
`roundtrip.py` `parsed_canon = canonical_fasm(...)`. Run dir held
only `blinky-xcku3p/emit.bit` and `roundtrip.bit`: no `diff.jsonl`,
no `manifest.json`.

Fix in `roundtrip.py`: `run_roundtrip` never raises. Failure kinds,
in order: golden FASM malformed goes blocked (the golden is at
fault, not the emitter); emitter subprocess failure (emit/parse exit
nonzero, not found, timeout) goes blocked, mirroring the emit_error
path in `diff_engine.diff_design`; emitter parse output that is not
valid FASM goes FRAMING red with one failed check
(`roundtrip-parse(emit(f))==f`) and the error named in the detail,
the same as a corrupt parse (valid FASM, wrong value), which is
already a red; anything else goes blocked with the exception named.
After the fix the same repro command exits 0, writes all 10 records
plus manifest, and the FRAMING record is red, `diff_bits` 1, detail
"emitter parse output is not valid FASM: emitter parse output:1: no
tile.feature separator in line 'this is not fasm at all'".

Regression tests (suite now 20 tests, all OK): the fake emitter
supports `FAKE_PARSE_GARBAGE=1` (parse prints the garbage line);
`test_garbage_parse_output_is_framing_red_not_crash` runs
`diff_design` with it and asserts 10 records, FRAMING red with
`diff_bits` 1 and the error named, LUT/FF/INT still green through
the bits path; `test_garbage_parse_emitter_full_run_writes_all_records`
runs `diff.py` with it over all 4 combos and asserts exit 0, no
traceback on stderr, 40 records, FRAMING red on all 4 combos, and a
manifest with 4 combos. Default run re-checked after the fix:
`runs/20261107T221652Z/`, exit 0, 40 records, all blocked.

## FF INIT completion in the round-trip check (diff-harness session 3)

CTO ruling, from the bitgen-core session 2 FLAG: the real emitter's
parse-back renders exactly one `.V0`/`.V1` line per FF in the device
for DBs with the pair structure (exact-complement rule), so
round-trip exactness (`parse(emit(f))==f`) requires complete FASM.
The Xilinx default FF INIT is 0, so an unspecified FF INIT means
INIT=0, which selects `.V0` (bit set). The emitter does not invent
defaults; the harness canonicalizes.

Change in `roundtrip.py` (the placement-hash canonical form in
`fasm.py` is untouched, so the golden hashes are unchanged):
`canonical_roundtrip_fasm(text, design)` parses the text, normalizes
spellings, completes every FF in the design with exactly one INIT
line (a missing FF gets the default `.V0`), sorts the lines, and
serializes. Normalization: valued FF INIT/SRVAL (`AFF.INIT = 0`,
`1'b0`) becomes the canonical `.V0`/`.V1` (the V suffix names the
value, bit = NOT(value) per the resolved polarity, researcher
session 1); Verilog-style values (`64'h...` as the real emitter's
parse-back renders LUT INIT, uppercase hex) become the golden's
lowercase `0x` spelling. Bare `AFF.INIT`, a valued `.V0`/`.V1` line,
and an FF value outside 0/1 raise FasmError (fail loud, never
guess). `run_roundtrip` takes the design name now (threaded from
`diff_design`) and compares both sides in this form. The design's
FF list comes from the golden design definition: new
`design_ff_locations(design)` in `gen_golden.py` (blinky: AFF;
counter: AFF, BFF, CFF, DFF; all in SLICE_X0Y0).

Semantics, stated honestly: a faithful emitter reproduces a partial
golden clean (both sides complete identically). The real emitter
renders `.V1` for an unmentioned FF (bit 0), so a partial golden
still fails against it, as a precise `.V0` vs `.V1` value mismatch:
the emitted bitstream genuinely encodes INIT=1 where the design
default is INIT=0. Only a complete golden can go green against the
real emitter. Completion is round-trip-only: the per-block diff
still sees the partial golden (a 3-of-4 FF counter golden reports
FF_INIT total_bits 3, not 4).

Regression tests (suite now 26 tests, all OK): new fake-emitter
mode `FAKE_PARSE_FF_FLIP=1` flips the AFF INIT value in parse
output only (the bits path reads the unflipped sidecar);
`test_partial_ff_init_roundtrips_clean_after_completion` runs
`diff_design` on a counter golden minus the DFF.INIT line with the
echo fake emitter and asserts FRAMING green, FF_INIT green,
total_bits 3;
`test_wrong_ff_init_value_still_fails_roundtrip` runs blinky with
the flip and asserts FRAMING red with `diff_bits` 1 while
LUT/FF/INT stay green;
`test_completes_missing_ff_init_with_v0` asserts the partial
counter canonicalizes identically to the full golden (completion is
idempotent) with all four `.V0` lines and no valued INIT lines;
`test_normalizes_valued_ff_init_and_verilog_lut_value` asserts the
`64'h` and `1'b0` spellings normalize and that shuffled input gives
the same form;
`test_real_emitter_parseback_spelling_matches_complete_golden`
asserts a complete golden and its `.V0`/`.V1` parse-back spelling
canonicalize identically, while a partial golden against a
`.V1`-rendering parse-back differs on the value;
`test_rejects_bare_ff_init_and_unknown_design` asserts FasmError
for bare `AFF.INIT`, valued `.V0`, value 2, and an unknown design.
Negative control: plain `canonical_fasm` on the partial counter
has no DFF.V0 line and keeps the valued spelling, and a complete
golden does not equal its parse-back spelling under it; the new
function fixes both.

Full run re-checked after the fix: `runs/20261107T225008Z/` (exit
0), 40 records, all blocked, all `diff_bits` null; placement hashes
identical to the baseline run and to the hashes recorded above
(blinky xcku3p b65979c1..., blinky xcvu3p 791ce945..., counter
xcku3p 1bf0ffe6..., counter xcvu3p 74417af5...). No-emitter default
run behavior is unchanged: `run_roundtrip` returns blocked before
any canonicalization when there is no emitter.

## Tile-scoped round-trip comparison and SRVAL completion (diff-harness session 4)

CTO rulings on the two flags session 3 left open.

1. Device-wide parse-back scope: the real emitter's parse-back
renders lines for every tile in the device model
(`parse_bitstream` iterates `device.tile_names()`), so the
round-trip comparison is now scoped to the design's tiles.
`run_roundtrip` computes the design's tile set from the golden
FASM itself (SLICE_X0Y0, INT_X0Y0, HPIO_RIGHT_X0Y0 for the v1
goldens) and passes it to `canonical_roundtrip_fasm` for the
parse-back side, which drops lines on tiles outside the set
before completing. The golden side is canonicalized without the
filter (its lines define the set). The per-block bits diff still
catches stray set bits anywhere, so scoping the FRAMING
comparison loses no coverage. New fake-emitter mode
`FAKE_PARSE_DEVICE_WIDE=1` appends the lines a real parse-back
renders for an untouched tile (every FF of SLICE_X9Y9, INIT.V1
and SRVAL.V1, bit 0 renders V1 through the exact-complement
rule).
2. SRVAL completion: same default completion as INIT.
`canonical_roundtrip_fasm` now completes every FF in the design
with exactly one SRVAL line as well as one INIT line; a missing
SRVAL gets the default `.V0` (Xilinx default SRVAL is 0). Valued
SRVAL spellings normalize to `.V0`/`.V1` like INIT. New
fake-emitter mode `FAKE_PARSE_SRVAL_V1=1` appends
`SLICE_X0Y0.AFF.SRVAL.V1` to the parse output.

Regression tests (suite now 30 tests, all OK; was 26, +4):
`test_completes_missing_ff_srval_with_v0` (counter golden gains
one `.SRVAL.V0` per design FF, valued SRVAL normalizes,
completion idempotent);
`test_device_wide_parseback_lines_outside_design_tiles_pass`
(blinky with device-wide lines: FRAMING green, FF/LUT/INT
green);
`test_wrong_value_on_design_tile_still_fails_with_device_wide_lines`
(device-wide lines plus a flipped AFF INIT: FRAMING red,
`diff_bits` 1, LUT/FF/INT green);
`test_wrong_srval_value_still_fails_roundtrip` (parse renders
AFF.SRVAL.V1 against a golden completed to .V0: FRAMING red,
`diff_bits` 1, LUT/FF/INT green through the bits path).

Full run re-checked after the fix: `python3 diff.py --out
/tmp/session4-default-run` (exit 0), 40 records, all blocked, all
`diff_bits` null; placement hashes identical to the baseline run
(blinky xcku3p b65979c1..., blinky xcvu3p 791ce945..., counter
xcku3p 1bf0ffe6..., counter xcvu3p 74417af5...). The placement-hash
canonical form in `fasm.py` is untouched, so the golden hashes are
unchanged.

Residual gap, recorded honestly and counted, not hidden: tile
scoping does not cover the lines parse-back renders for unused
BELs and attribute pairs on the design's OWN tile. Counted from
the real `segbits_clel_l.db` (768 features): 57 all-`!` features
render on an untouched CLEL_L tile through the exact-complement
rule (16 INIT.V1 + 16 SRVAL.V1, one per FF BEL, plus CEUSED 4,
CLKINV 2, SRINV 2, SRUSED 4, MODE 2, SYNC 2, CARRY_TYPE 1,
DI0-DI7 8). The completed golden carries only the design's FFs
(1 INIT + 1 SRVAL for blinky, 4 + 4 for counter), so a
real-emitter FRAMING run still needs the next CTO ruling: either
scope the comparison to the design's features, or complete the
golden tile-wide with attribute defaults. This session implements
the tile scope and SRVAL completion as ruled; it does not claim
the real-emitter FRAMING run is green yet.

## Feature-scoped FRAMING comparison (diff-harness session 5)

CTO ruling on the flag session 4 left open: scope the FRAMING
comparison to the design's features (the alternative, completing
the golden tile-wide with attribute defaults, was not chosen).

Change in `roundtrip.py`: after canonicalization, both sides are
filtered to the design's feature set, the golden's features plus
the completed FF INIT/SRVAL defaults. New helpers:
`_feature_key(feat)` keys a canonical feature by (tile,
attribute), the `.V0`/`.V1` suffix stripped, because the suffix
IS the value (V0 = value 0, V1 = value 1); every other feature
keys on its full feature string and its value is compared
separately. `roundtrip_feature_keys(text)` returns the key set
of a canonical text. `scope_fasm_to_feature_keys(text, keys)`
parses, drops lines whose key is not in the set, sorts, and
serializes. `run_roundtrip` computes the golden's keys from the
completed golden canonical form and scopes both sides through
it; scoping the golden to its own keys is a no-op by
construction, stated in the code and proven by test. The
parse-back side is still tile-scoped first (session 4), then
feature-scoped. The per-block bits diff still catches stray set
bits anywhere, so the scoping loses no coverage: a stray bit for
an unspecified attribute shows up in the bits diff as an extra
set bit, never in FRAMING.

The 57-line count was re-verified this session, not taken on
trust: a script implementing the exact-complement rule
(all-`!` feature renders iff its zero set equals another
feature's set-bit set with no `!` bits) over the real
`segbits_clel_l.db` returns exactly 57 lines, matching the
session-4 breakdown line for line (16 INIT.V1 + 16 SRVAL.V1,
including the FF2 BELs, plus CEUSED 4, CLKINV 2, SRINV 2,
SRUSED 4, MODE 2, SYNC 2, CARRY_TYPE 1, DI0-DI7 8 as
`.AX`/`.BX`/`.CX`/`.DX`/`.EX`/`.FX`/`.GX`/`.HX`).

Regression tests (suite now 34 tests, all OK; was 30, +4).
New fake-emitter mode `FAKE_PARSE_OWN_TILE_NOISE=1` appends the
lines a real parse-back renders for unused BELs and attribute
pairs on the design's own tile (unused FF BELs EFF..HFF and
AFF2..HFF2 INIT.V1/SRVAL.V1, ABCDFF/EFGHFF CEUSED/CLKINV/SRINV/
SRUSED/MODE/SYNC, CARRY_TYPE.SINGLE_CY8, DI0.AX..DI7.HX, all
spellings verbatim from `segbits_clel_l.db`), plus the session-4
device-wide lines for SLICE_X9Y9:
`test_feature_scoped_comparison_drops_own_tile_noise`
(canonical level: own-tile noise plus other-tile lines scope
away to exactly the completed golden; scoping the golden to its
own keys is a no-op; a wrong AFF INIT value survives the scope
because V1 shares the (tile, AFF.INIT) key with the golden's
V0);
`test_own_tile_noise_lines_outside_design_features_pass`
(blinky and counter with the noise: FRAMING green, FF/LUT/INT
green);
`test_wrong_value_on_design_feature_still_fails_with_own_tile_noise`
(noise plus a flipped AFF INIT: FRAMING red, `diff_bits` 1,
LUT/FF/INT green);
`test_stray_set_bit_outside_design_features_caught_by_bits_diff`
(noise plus a bit set in DSP48E2, a block the design does not
use: FRAMING green, DSP48E2 red with `diff_bits` 1, LUT/FF/INT
green).

Full run re-checked after the fix: `python3 diff.py --out
/tmp/session5-default-run` (exit 0), 40 records, all blocked, all
`diff_bits` null; placement hashes identical to the baseline run
(blinky xcku3p b65979c1..., blinky xcvu3p 791ce945..., counter
xcku3p 1bf0ffe6..., counter xcvu3p 74417af5...), verified
full-hash against VERIFICATION.md, all four MATCH. The
placement-hash canonical form in `fasm.py` is untouched, so the
golden hashes are unchanged.

What this session does NOT claim: the real-emitter FRAMING run
is green yet. The feature scope closes the last known gap in the
comparison itself (own-tile complement lines); the run still
awaits the bitgen-core merge plus the integration worker's
emitter executable (critic session 9 FLAG F1).

## Independent hand verification

Command: `python3 tests/hand_verify_lut.py`

Result: `HAND VERIFICATION PASSED`. The script reads
`reference/ULTRASCALE_PLUS.json` and `reference/xcku3p.json` directly
with `json` only, applies the FAR layout from UG570 table 9-21 by
hand, and compares all 64 blinky A6LUT bits against the harness
resolver: exact match. It also recomputes `diff_bits` by hand with a
plain loop for 3 flipped bits: 3 by hand, 3 by the harness.

## End-to-end runs

Command: `python3 diff.py`

Final run: `runs/20261107T220702Z/` (exit 0). Cross-check PASS
(BitFiltrator vs prjuray, SLICE_X0Y0: 4 LUTs x 64 bits, 0 mismatches;
4 FFs, V0/V1 complement). 40 JSONL records (10 blocks x 4 combos),
ALL blocked, all `diff_bits` null, every record carries seed
20261107 and its placement hash. Manifest: tier `tier1`, emitter
null, reference_bit null, 4 combos.

Placement hashes (regenerated with the fixed hash that binds the
FASM text, sha256 over placement record + canonical FASM):

- blinky xcku3p: b65979c112456c7049770e0fd1573939148c901606eef9f7a0b8a0f74fddacfb
- blinky xcvu3p: 791ce94587b0ab2074fe211a91731584846b9a71cae189cee0d8a19ff4e9dffb
- counter xcku3p: 1bf0ffe68ca860b2a3b7c05b56e79fc5e7f8e7949af49bb7504734a68ba012ea
- counter xcvu3p: 74417af567417cdac387ade31bd081c6ea632f09556f5235a1a6ff7d6bd3e23c

Greedy queues (largest first) from the final run:

- blinky/xcku3p: LUT_INIT 64, IO_CONFIG 55, INT_PIP 11, FF_INIT 1,
  then BRAM_CONTENT/BRAM_PARITY/CLK/DSP48E2/FRAMING/URAM288 at 0.
- counter/xcku3p: LUT_INIT 256, IO_CONFIG 55, INT_PIP 17, FF_INIT 4,
  rest 0.
- blinky/xcvu3p and counter/xcvu3p: IO_CONFIG 55 first; LUT/FF/INT
  total 0, blocked with missing "xcvu3p device file not yet
  generated (device-file worker)"; BRAM/DSP/URAM 0 not exercised;
  CLK 0 not exercised; FRAMING 0 blocked on missing emitter.

Reference totals on xcku3p, per record: blinky LUT_INIT 64, FF_INIT
1, INT_PIP 11, IO_CONFIG 55; counter LUT_INIT 256, FF_INIT 4,
INT_PIP 17, IO_CONFIG 55. Every blocked record names its missing
piece; the dominant one is the absent bitgen-core emitter:
"bitgen-core emitter absent: no --emitter-cmd and
HELION_BITGEN_EMITTER unset; emit_bitstream/parse_bitstream not yet
implemented".

## Tier-2 slot check

Command: `python3 diff.py --design blinky --part xcku3p
--reference-bit /tmp/dummy-ref.bit --out /tmp/tier2check`

Result: tier `tier1+tier2`; manifest `tier2` entry records the file
and status `blocked` with missing "tier-2 diff needs the emitter to
parse a Vivado bitstream (bitgen-core parse_bitstream on a Vivado
.bit); no emitter is configured, so no tier-2 comparison ran". No
tier-2 comparison was faked.

Missing `--reference-bit` file: exit 2 (covered by unit test).

## Tamper rejection

Covered by unit test: editing the golden FASM after generation makes
`diff.py` exit 3 with REJECTED (placement hash binds the FASM text).

## Repo cleanliness

`git status` in `~/workspace/helion-ultra-bitgen-diff` (branch
fm/ultra-bitgen-diff, base aee77e3): clean. The harness lives outside
the repo at `~/workspace/ultra-bitgen-crew/harness/`; no repo files
were edited.

## Known limitations (documented, not hidden)

- INT PIP absolute FARs use the provisional INT col = CLB col - 1
  rule (derived from the xczu3eg tilegrid); device-file worker owns
  confirming it for xcku3p/xcvu3p.
- IO/CLK reference bits are tile-relative until the xcku3p tilegrid
  lands; absolute comparison stays blocked.
- FRAMING has no independent .bit byte decoder yet beyond sync word
  + IDCODE checks; round-trip is the check until a UG570 packet
  decoder is built.
- CLK, DSP48E2, URAM288 have no encoding in either public source;
  they need Vivado fuzzing (license blocked) or new public data.
- BRAM content/parity resolve, but the golden designs exercise no
  BRAM; a BRAM golden design is needed before those blocks can go
  green.
