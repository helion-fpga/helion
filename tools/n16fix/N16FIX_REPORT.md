# N16 fix report: per-bit-subset ambiguity acceptance in `check_stored_band`

Branch: `fm/ultra-bitgen-n16fix`, base `9c4917de`, worktree
`~/workspace/helion-ultra-bitgen-n16fix`. No merges. No critic
dispatched by this lane. 0 em dashes in this report, in the code,
or in the tests.

## Verdict

N16 is fixed for small ambiguity sets and honestly bounded for the
rest. The pinned test `mixed_column_ram_plus_logic_false_rejects`
is un-ignored, renamed `mixed_column_ram_plus_logic_parses_clean`,
and passes. A dense mixed column still fails loud, pinned by a new
test. Dense real mixed columns (rank 40 to 44 on 3,863 of 3,948
ambiguous corpus frames) still fail loud too; that is the
documented residual, not a silent accept.

## Fail-before

Command:

```
cd ~/workspace/helion-ultra-bitgen-n16fix && /home/hatch/.cargo/bin/cargo test -j2 -p helion-ultrascale --test n7_probe_mixed_column mixed_column_ram_plus_logic_false_rejects -- --ignored --exact
```

Log: `~/workspace/ultra-bitgen-crew/logs/n16fix-fail-before.log`.
Output (tail):

```
Finished `test` profile [unoptimized + debuginfo] target(s) in 2m 28s
 Running tests/n7_probe_mixed_column.rs (target/debug/deps/n7_probe_mixed_column-ec785416b0f53d55)

running 1 test
test mixed_column_ram_plus_logic_false_rejects ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.79s
```

The ignored test passed in its old form, which asserts the bug:
parse returns `BandMismatch` on FAR `0x60b`, stored
`0x088000008088`, expected `0`. The false reject reproduces.
(First spawn attempt failed with `setsid: failed to execute cargo:
No such file or directory` because `cargo` is not on PATH in the
exec shell; reran with the absolute path
`/home/hatch/.cargo/bin/cargo`.)

## The disproven two-extremes rule, confirmed

From `decoder-validation/h1_ecc_sparse_maps.json` key "A":
`col(15) = 0x880800088088`, `col(975) = 0x088000008088`. The stored
pinned band `0x088000008088` equals `col(975)` exactly. Fully
masked predicts `0`; fully unmasked predicts
`col(15)^col(975) = 0x808800080000`. Neither extreme matches, so
accepting either extreme does not close the pinned test. The
correct ambiguity set is per-bit subsets of the ambiguous masked
set bits, solved by Gaussian elimination over the 48-bit column
vectors. FAR minor 11 (`0x60b`) holds letter-A INIT bits for all
60 slices at distinct frame offsets (Y0 bit 0 at offset 15, Y20
bit 0 at offset 975, 960 apart; confirmed from
`ultrascale_plus_lut_ff_bram.json` LutLoc Y_ofst).

## Measurement: how big is the ambiguous set in practice

Script: `~/workspace/ultra-bitgen-crew/logs/n16fix_measure3.py`
(v1 unioned LutLoc positions across tile types and inflated the
mask; v2 fixed that per tile type; v3 fixed the group key to
(slice, letter)). It rebuilds the parse over-mask per corpus frame
(any letter with MODE.RAM anywhere in the column masks all that
letter's INIT positions in the frame) over all 14 frame dumps
(144,816 block-0 frames) with the true per-slice masks from
`~/workspace/n7-band-check-masks/*.masks.bin`. No pickle files
were read; inputs are `.bin` (struct) and `.json` only.

Log: `~/workspace/ultra-bitgen-crew/logs/n16fix_measure3.log`.
Results:

- Block-0 frames: 144,816. Frames in a RAM-letter column: 5,584.
  Of those, 3,948 carry at least one ambiguous masked set bit;
  1,636 carry none (already Exact).
- Ambiguous masked set bits per frame (`n_amb`): 0 on 1,636 frames,
  1 to 8 on 1 frame, 9 to 32 on 9 frames, 33 to 128 on 77 frames,
  129 to 512 on 1,591 frames, 513+ on 2,270 frames. Maximum 1,442.
- Per-bit column rank: 40 to 44 on 3,863 frames, maximum 44,
  never 48. The whole MAP-A table spans only a 44-dimensional
  subspace of the 48-bit band (rank of all 2,927 columns is 44),
  so at rank 44 a random wrong band lands in the span with
  probability 1 in 16. No bound near the full rank is honest.
- Per-bit subset acceptance would accept all 5,584 RAM-column
  frames (5,582 in-span, 2 with Q, 0 not-in-span), which is exactly
  why it must be bounded: unbounded, the check is vacuous on
  dense frames.
- Acceptance ladder (frames with `n_amb > 0` accepted at per-bit
  rank at most T): T=8: 1 frame; T=16: 5; T=20: 10; T=24: 20;
  T=28: 26; T=32: 36; T=40: 232; T=44: 3,948.
- The true subset is a union of (slice, letter) groups: per-group
  in-span is 5,582, with Q 2, not-in-span 0, confirming the group
  structure. Group rank at most 24 holds on 801 frames, at most 32
  on 1,601. (v2 keyed groups by slice Y only and wrongly rejected
  4 frames; the (slice, letter) key fixes that.)
- Q is in the span of all MAP-A columns (rank-44 basis reduces it
  to 0), so the Q-combined subset variant adds nothing once the
  ambiguous rank reaches 44, and matters only for small sets.

## The bound

`MAX_AMBIGUOUS_RANK = 24` in `crates/helion-ultrascale/src/band_code.rs`.

- The accepted set holds at most 2^24 deltas (2^25 with the
  Q-combined variant), so a uniformly random wrong band passes
  with probability at most 1 in 8,388,608. Over the 144,816-frame
  corpus that is under 0.02 expected slips.
- 24 is the largest bound keeping that corpus-scale expectation
  below 0.02 while accepting four times the frames rank 16 does
  (20 vs 5). At rank 32 the expectation would be about 2.2 slips
  per corpus, too weak for a correctness check.
- The bound is on rank, not count: rank is what sizes the
  accepted set. Count is unbounded but irrelevant to safety
  (dependent columns add no new deltas) and cheap to process
  (one pass over the 2,927 MAP-A entries, only on frames that
  already failed Exact and QResidual).
- Consequence, stated plainly: this fix closes the pinned test
  and 20 of 3,948 real ambiguous frames. The other 3,928 real
  frames still fail loud. A group-aware variant would accept 801
  real frames at the same rank bound, but `check_stored_band`
  receives no grouping input, so that stays future work, not
  implemented here.

## Adjudication: the one failing test in full-suite run 2

Full-suite run 2 (`~/workspace/ultra-bitgen-crew/logs/n16fix-fullsuite2.log`,
exit 101) had exactly one failure:
`mixed_column_with_unmasked_band_fails_loud` in
`crates/helion-ultrascale/tests/n7_decode_mixed_column.rs`, a
pre-existing test NOT modified by this lane. It called `unwrap_err()`
on a decode that now returns `Ok` with FASM
`[SLICE_X0Y0.A6LUT.INIT[63:0] = 0x1, SLICE_X0Y0.ALUT.MODE.RAM,
SLICE_X0Y20.A6LUT.INIT[63:0] = 0x1]`, unexplained `[]`.

Adjudication, from the evidence: the stored band is
`col(15)^col(975)` (unmasked, both INIT columns); the decoder masks
only the Y0 INIT bit, expects `col(975)`, and the delta is `col(15)`
alone, rank 1, inside the bound of 24. The decoded FASM is correct
and complete with zero unexplained bits, identical to the passing
masked case `mixed_column_ram_plus_logic_decodes_clean`. This
acceptance is the intended N16 semantics, not a silent error; the
test's old fail-loud expectation is the N16 false-reject behavior on
the decode side. `check_stored_band` was NOT weakened, the bound was
NOT raised, and the dense-fails-loud test was NOT touched.

The test was updated to the new semantics: renamed to
`mixed_column_with_unmasked_band_decodes_clean`, asserting the exact
FASM above and empty unexplained. A decode-level corrupted-band test
was added in the same file,
`mixed_column_with_corrupted_band_fails_loud`: the 0x60b band is
computed with only the Y0 INIT bit masked (expected `col(975)`,
subset XORs `col(975)` and `col(975)^col(15)`), then band bit 0 is flipped, so
the stored band is `col(975)^1`, one bit off any subset XOR, with or
without Q. Decode fails loud with `BandMismatch` naming FAR `0x60b`.
That test is now the pin that the band check still fails loud.

## Implementation

Files changed (8), branch `fm/ultra-bitgen-n16fix`:

- `crates/helion-ultrascale/src/band_code.rs`: new `BandCheck`
  variants `AmbiguousSubset` and `AmbiguousSubsetQResidual`;
  `MAX_AMBIGUOUS_RANK = 24` with the measurement documented;
  `check_stored_band` now, after Exact and QResidual fail on a
  block-0 frame, collects the MAP-A columns of the ambiguous
  masked set bits, builds a GF(2) basis keyed by highest set bit,
  fails loud with `Mismatch` when the rank exceeds 24, and
  otherwise accepts when the stored-minus-expected delta, or
  that delta XOR `Q_RESIDUAL`, reduces to zero by the basis.
  Block-1 and block-2+ frames are unchanged (expected 0, no
  subset acceptance). New unit tests below.
- `crates/helion-ultrascale/src/bin/band_check.rs`: the corpus
  binary counts the two new variants separately (they must stay 0
  under the true per-slice masks) and prints them.
- `crates/helion-ultrascale/src/bitgen.rs`: parse band-check
  comment updated to name the subset acceptance. No behavior
  change beyond `check_stored_band`.
- `crates/helion-ultrascale/src/bitstream_decode.rs`: module and
  call-site comments updated the same way. No behavior change
  beyond `check_stored_band`. Both callers match only
  `BandCheck::Mismatch`, so the new variants pass there as
  designed.
- `crates/helion-ultrascale/tests/n7_probe_mixed_column.rs`:
  pinned test un-ignored and renamed to
  `mixed_column_ram_plus_logic_parses_clean` (mixed case parses,
  both INIT lines and MODE.RAM present); new
  `mixed_column_dense_still_fails_loud` (below).
- `crates/helion-cli/src/ultra.rs`: the two flow tests (blinky,
  counter) now panic on the new `AmbiguousSubset` and
  `AmbiguousSubsetQResidual` variants too; with an all-false mask
  there is nothing ambiguous, so every frame must still check Exact.
  Comment-only change apart from the two new match arms.
- `crates/helion-ultrascale/tests/band_code.rs`: the corpus-sample
  test counts the two new variants separately (`amb`, `amb_q`) and
  asserts both are 0 under the true per-slice fixture masks.
- `crates/helion-ultrascale/tests/n7_decode_mixed_column.rs`:
  adjudication edit, see the Adjudication section above: the
  unmasked-band test renamed to
  `mixed_column_with_unmasked_band_decodes_clean` with exact-FASM
  and empty-unexplained assertions, plus the new
  `mixed_column_with_corrupted_band_fails_loud`.

One pre-existing unit test changed semantics deliberately:
`check_stored_band_accepts_exact_and_q_and_rejects_other` used
to assert that a single masked set bit with the stored band equal
to its column is a `Mismatch`. That case is exactly the N16
ambiguity (the bit may be a logic INIT bit, rank 1), so it now
expects `AmbiguousSubset`, with a comment saying so. Corrupted
values are still mismatches, pinned by the new dedicated tests.

## Regression tests

New unit tests in `band_code.rs`:

- `check_stored_band_accepts_ambiguous_subsets`: pins
  `col(15) = 0x880800088088` and `col(975) = 0x088000008088`
  against the Rust tables, then accepts every subset of the two
  (0, either column alone, both), including the single-column
  stored band the two-extremes rule misses, plus the
  subset-plus-Q variant, while bare Q stays `QResidual`.
- `check_stored_band_rejects_corrupted_band_under_ambiguity`:
  the same frame with the stored band one bit off any subset XOR
  (`col(975)^1`, `0x123456789abc`, subset-plus-Q-plus-1) is a
  `Mismatch` naming expected 0.
- `check_stored_band_rejects_over_bound_ambiguous_set`: sources
  0..96 carry 97 masked set bits of rank 25 (first prefix of the
  Rust MAP-A table to reach rank 25), one above the bound. A
  stored band that IS a subset XOR of those columns still fails
  loud, with and without Q; the same stored band with only the
  three subset bits masked (rank 3) is accepted. The bound, not
  the value, decides.

New integration test in `n7_probe_mixed_column.rs`:

- `mixed_column_dense_still_fails_loud`: RAM on Y0 letter A plus
  all-ones logic INIT on Y20..Y29 letter A. Every INIT frame
  carries 160 ambiguous masked set bits of rank 32 and must fail
  loud as `BandMismatch`. One asymmetry is deliberate and
  documented in the test: the 16 columns of one slice in one minor
  XOR to zero, so a symmetric all-ones design carries a zero band
  on every INIT frame and parses clean despite the rank; clearing
  Y29 INIT bit 0 makes the minor-11 band `0x888008880888`,
  nonzero, while the rank stays 32. The first version of this test
  used symmetric all-ones and passed parse for exactly that
  reason; the failure output showed the parsed FASM with a zero
  band, which is how the cancellation was found and measured.

New integration tests in `n7_decode_mixed_column.rs` (adjudication,
see above):

- `mixed_column_with_unmasked_band_decodes_clean` (renamed from the
  old fail-loud test): the unmasked small-ambiguity band now decodes
  clean with the exact three-line FASM and zero unexplained bits.
- `mixed_column_with_corrupted_band_fails_loud`: the 0x60b stored
  band one bit off any subset XOR still fails loud with
  `BandMismatch` naming FAR `0x60b`.

## Acceptance

Full-suite run 3, after the adjudication edit. Command:

```
cd ~/workspace/helion-ultra-bitgen-n16fix && /home/hatch/.cargo/bin/cargo test -j2 -p helion-ultrascale
```

Log: `~/workspace/ultra-bitgen-crew/logs/n16fix-fullsuite3.log`.
Result: 481 passed, 0 failed, across 35 test binaries plus
doc-tests (36 `test result:` lines, all `ok`). Run 2 had 428
passed and exactly 1 failed, the adjudicated decode test above.
N16-relevant binaries in run 3:

```
test result: ok. 265 passed; 0 failed (lib)
test result: ok. 2 passed; 0 failed (n7_probe_mixed_column)
test result: ok. 4 passed; 0 failed (n7_decode_mixed_column)
test result: ok. 6 passed; 0 failed (band_code)
```

Targeted decode run before the full suite (log
`n16fix-decode-test.log`): 4 passed, 0 failed, including the renamed
unmasked-band test and the new corrupted-band test.

Corpus band_check re-run, same arguments as the first run. Command:

```
cd ~/workspace/helion-ultra-bitgen-n16fix && /home/hatch/.cargo/bin/cargo run -j2 -q -p helion-ultrascale --bin band_check -- --validation-dir /home/hatch/workspace/ultra-bitgen-crew/decoder-validation --masks-dir /home/hatch/workspace/n7-band-check-masks
```

Log: `~/workspace/ultra-bitgen-crew/logs/n16fix-bandcheck2.log`,
exit 0. Counts unchanged from the first run:

```
block0 frames: 144816
MAP-A masked exact: 144332
MAP-A residual w (0xf0000000000f): 484
MAP-A residual other: 0
MAP-A ambiguous subset (N16): 0
MAP-A ambiguous subset plus w (N16): 0
H76 exact: 143269
H76 residual w: 433
H76 residual other: 1114
block1 frames: 64512
block1 nonzero band: 0
other block-type frames: 0
```

Fmt status: `cargo fmt --check -p helion-ultrascale` exits 1 with
128 diffs (log `n16fix-fmt3.log`), crate-wide baseline failure, NOT
reformatted. Every diff inside the 8 touched files was checked
against the lane's changed hunks: none overlaps a lane-touched
line. The touched-file diffs are pre-existing baseline (for
example `band_check.rs:25` import order, `bitgen.rs` and
`bitstream_decode.rs` diffs at lines the lane did not touch,
`tests/band_code.rs:8` import block). The adjudication edit in
`n7_decode_mixed_column.rs` adds zero fmt diffs. No fmt nit
introduced by this lane remains.

Em dashes: 0 in the report, 0 in the final diff (verified by grep
over `git diff`).

## Residuals and follow-ups

- Dense real mixed columns (rank 40 to 44, 3,863 frames) still
  fail loud on parse. The unblocker is group-aware acceptance:
  the true subset is a union of (slice, letter) groups, and group
  rank 24 or below holds on 801 real frames. That needs grouping
  input threaded through `check_stored_band` and both callers,
  plus the corpus binary; not done here.
- MODE.RAM still renders on every slice tile of a mixed column in
  the parsed FASM (F1); only the band verdict changed. The
  decoded FASM therefore lists a spurious MODE.RAM line on logic
  slices. Out of scope for N16.
- `cargo fmt --check -p helion-ultrascale` fails on this tree at
  baseline (128 diffs under rustfmt 1.99.1, including untouched
  files such as `tileconn.rs`, `device_file.rs`, and the generated
  `band_code_tables.rs`). Every diff inside the 8 touched files was
  checked against the lane's changed hunks and none overlaps a
  lane-touched line: the touched-file diffs are pre-existing
  baseline (for example the `band_check.rs` import order at line
  25, `bitgen.rs` and `bitstream_decode.rs` diffs at lines this
  lane did not touch, the `tests/band_code.rs` import block at
  line 8). The two fmt nits this lane introduced (a multi-line fn
  signature in `band_code.rs`, import order in the probe test)
  were fixed, and the adjudication edit in
  `n7_decode_mixed_column.rs` adds zero fmt diffs. The crate-wide
  reformat is out of scope and was not attempted.
- No new compiler warnings in the touched files: the test logs
  show only pre-existing warnings (`bitstream_packets.rs`
  dead-code constants, `helion-sv`, `helion-ir`).

## Commands and outputs

All cargo commands use `/home/hatch/.cargo/bin/cargo` with `-j2`
maximum, logs under `~/workspace/ultra-bitgen-crew/logs/`.
