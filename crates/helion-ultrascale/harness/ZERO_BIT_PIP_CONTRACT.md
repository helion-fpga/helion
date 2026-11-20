# Zero-bit PIP round-trip contract (F4)

Status: DECIDED. Lane: bitgen-core F4, branch `fm/ultra-bitgen-core-f1f4`,
base `a11407cf`. This doc is the contract; `roundtrip.py` implements it
and `tests/test_harness.py` (`TestZeroBitPip`) pins it.

## The finding

The 5 FRAMING reds in the US-07 100-design loop (rand057, rand073,
rand079, rand085, rand097) share one cause: each design contains a
VCC_WIRE PIP whose segbits entry lists must-be-zero bits only, so the
feature sets zero bits and parse-back cannot render it. INT_PIP stays
green on all five designs. The exact records (crew harness
`runs/us07-batch*/diff.jsonl`):

| design | VCC_WIRE line | FRAMING record |
|---|---|---|
| rand057 | `INT_X87Y0.PIP.IMUX_W23.VCC_WIRE` | red, diff 1, total 3, "round-trip red: 2 passed, 1 failed, 0 skipped" |
| rand073 | `INT_X86Y0.PIP.IMUX_E23.VCC_WIRE` | red, diff 1, total 3, same detail |
| rand079 | `INT_X62Y0.PIP.IMUX_E16.VCC_WIRE` | red, diff 1, total 3, same detail |
| rand085 | `INT_X45Y0.PIP.IMUX_E24.VCC_WIRE` | red, diff 1, total 3, same detail |
| rand097 | `INT_X98Y0.PIP.BYPASS_E10.VCC_WIRE` | red, diff 1, total 3, same detail |

Reproduced on this branch before the fix with the worktree harness and
the release `helion-bitgen` built from this base: rand057 FRAMING red,
diff 1, total 3, INT_PIP green, diff 0, total 10. The canonical
golden (44 lines) and the canonical parse-back (43 lines) differ in
exactly one line: `INT_X87Y0.PIP.IMUX_W23.VCC_WIRE`, present in the
golden, absent from parse-back. No other line differs.

## The DB facts

`segbits_int.db` (prjuray-db, zynqusp, pinned commit
affbc5e555ebae16475f32e8fb2d6565d4204f3f) carries 146 VCC_WIRE lines:

- 145 list must-be-zero bits only. All five failing features are in
  this set, each 6 `!` tokens, 0 set bits:
  - `INT.PIP.IMUX_W23.VCC_WIRE !01_11 !03_10 !03_11 !07_10 !09_10 !11_10`
  - `INT.PIP.IMUX_E23.VCC_WIRE !67_10 !69_10 !71_10 !73_11 !75_10 !75_11`
  - `INT.PIP.IMUX_E16.VCC_WIRE !64_02 !66_02 !70_00 !72_02 !74_01 !74_02`
  - `INT.PIP.IMUX_E24.VCC_WIRE !64_14 !66_14 !70_12 !72_14 !74_13 !74_14`
  - `INT.PIP.BYPASS_E10.VCC_WIRE !64_30 !66_30 !68_30 !72_30 !72_31 !74_30`
- Exactly 1 has a set bit, the exception the normalization report
  flagged: `INT.PIP.CTRL_W5.VCC_WIRE 21_35 !36_32 !36_33 !37_32 !37_33
  !38_32 !39_32 !46_31 !47_33`. It sets bit 21_35, so it IS
  observable, and none of the five failing designs uses it. The F4
  premise holds for the five designs; the exception is real and the
  contract honors it (see "Not unobservable" below).

Structural fact behind the class: every INT PIP feature in
`segbits_int.db` lists at least one `!` bit (3633 have both set and
`!` bits, 145 are all-`!`, 0 are pure set-bit). So for tile type INT
the exact-complement set is EMPTY: no all-`!` INT feature can ever
have a complement. All 145 all-`!` INT features (all VCC_WIRE) are
unobservable on parse-back. A normal PIP sets exactly one bit and
clears the rest of its mux select bits (for example
`INT.PIP.IMUX_E9.INT_NODE_IMUX_19_INT_OUT1` sets 71_26 and clears
five others); VCC_WIRE sets none and clears six: it is the mux
"select nothing, tie to VCC" default, all select bits 0, which is
indistinguishable from an unconfigured mux.

## Definitions

A valueless segbits feature is **unobservable** when its DB entry
lists zero set bits (every token is `!`) AND its zero signature (the
sorted (frame, bit) pairs of its `!` bits) is NOT the set-bit
signature of any feature of the same tile type that lists no `!`
bits (no exact complement). This mirrors the presence rule in
`bitgen.rs` `parse_bitstream` exactly: a valueless feature renders
only when all its bits agree AND (it sets at least one bit OR it has
an exact complement). The decoder (`bitstream_decode.rs`, same rule
plus the F2 owned-set-bit gate on complement defaults, commit
253d7954) agrees: an all-`!` feature with no complement is dropped.

Not unobservable, and therefore still compared exactly:

- Any feature with at least one set bit, including
  `INT.PIP.CTRL_W5.VCC_WIRE` (sets 21_35).
- Any all-`!` feature with an exact complement, including FF
  `.V1` lines (`CLEL_L.AFF.INIT.V1 !12_02` complements
  `CLEL_L.AFF.INIT.V0 12_02`).
- Any valued feature. A valued feature whose value is all zero is a
  different class (any-set presence in `bitgen.rs`) and is OUT OF
  SCOPE for this contract; it is not dropped by this change.

## Decision (chosen)

**Exclude unobservable features from the round-trip FASM comparison
on both sides, and verify them at the bit level.** Concretely:

- Emit: unchanged. The emitter sets no bits for the feature (there
  are none to set) and must leave every listed `!` bit at 0. Emit is
  correct; there is no emitter defect here.
- Parse-back: unchanged. The emitter and the decoder drop the
  feature because it is indistinguishable from absent. Parse-back is
  correct; there is no emitter or decoder change in this contract.
- Diff treatment: FRAMING compares canonical FASM with unobservable
  features removed from the design's feature set before comparing,
  so both sides are compared without them. Each removed feature is
  named in the FRAMING record's `checks_skipped` list as
  `roundtrip-unobservable:<tile>.<feature>`, so the green is
  auditable, never silent. The per-block bit diff still constrains
  every listed bit of the feature (value 0) and goes red if the
  emitter sets any of them: INT_PIP is the covering check. A design
  containing only such a feature (besides FF defaults) goes FRAMING
  green with the exclusion recorded, and INT_PIP green exactly when
  all its must-be-zero bits read 0. No harness-artifact red remains.

The round-trip contract is therefore: `parse(emit(f)) == f` holds for
every OBSERVABLE feature of `f`, and every unobservable feature of
`f` is verified to set no bits by the per-block bit diff. The old
statement of the contract (exact equality over all of `f`) is
unachievable in principle for this class: no bitstream can encode a
feature that sets no bits, so no correct emitter can round-trip it.

## Rejected alternatives

- Blocked-not-exercised (FRAMING blocked when a design contains an
  unobservable feature). Rejected: the comparison DID run, and
  blocked means "did not run" (a comparison that did not run must
  not look like a pass, and a comparison that ran must not look like
  it did not run). Blocking the whole FRAMING record would also hide
  genuine reds on the same design: a design can carry both a
  VCC_WIRE and a real round-trip defect, and the defect must still
  fail loudly.
- Silent exclusion from the FRAMING comparison with no covering check
  named. Rejected: it is the chosen mechanism minus the
  verification. Without the bit-level check, an emitter that set a
  must-be-zero bit for a VCC_WIRE feature (a real bug) would still
  report FRAMING green. The chosen contract names INT_PIP as the
  check that covers the excluded features.
- Change the emitter to render VCC_WIRE on parse-back. Rejected:
  the `bitgen.rs` presence rule is correct and pinned by its own
  tests ("A mux default such as VCC_WIRE has no complement and is
  dropped; otherwise every all-`!` feature of every tile would be
  emitted for unconfigured frames"). Rendering it would invent about
  145 spurious VCC_WIRE lines per unconfigured INT tile. It is also
  outside this lane: `bitgen.rs` is owned by Worker A, and the
  decoder already agrees with the drop (F2 precedent).
- Generator-side avoidance (stop generating VCC_WIRE designs).
  Rejected: the designs are valid FASM and the harness must handle
  valid input. The class is 145 features, not 5 designs; hiding
  them from the generator would leave the contract hole open for
  the next design that draws one.

## Scope and follow-ups

- Implemented for INT PIPs only: the proven case. The helper
  (`is_unobservable_feature`) is stated generally for any valueless
  segbits feature, but no other block is wired to it yet. Wiring
  CARRY8, LUTRAM, HPIO, CMT, or RCLK features through the same
  helper needs their DB key mapping (tile-type dependent) and a
  failing design that exhibits one; do not wire speculatively.
- If a future emitter ever renders an unobservable feature, FRAMING
  does not flip: the feature is outside the compared feature set on
  both sides, and the bit diff still adjudicates its bits.
- Grounding note: the harness judges observability from its
  reference `segbits_int.db`. Its data lines are byte-identical to
  the emitter's vendored `crates/helion-ultrascale/data/segbits_int.db`
  (verified by diff: only 4 header comment lines differ). If the two
  ever diverge, the harness could misjudge observability; the
  fail-safe is that an unresolvable feature is never dropped.
- Fail-safe: when the reference DB cannot be loaded, or a feature
  has no DB entry, the feature is treated as observable and the
  strict comparison stands. The fix never converts an unknown into
  a pass.

## Verification

- Unit: `python3 -m unittest discover -s tests` from the harness
  dir; `TestZeroBitPip` covers the helper, the five failing
  features against the real DB, the CTRL_W5 exception, the FF V1
  complement, and end-to-end FRAMING green/red through the fake
  emitter (which drops a named parse-back line, mimicking the real
  emitter's drop of an unobservable feature).
- Real emitter: `/tmp/f4_verify.py` (driver, not committed) runs
  `diff_design` over the five generated designs with the release
  `helion-bitgen` from this branch and prints the FRAMING and
  INT_PIP records. Before the fix rand057 reproduced the loop
  record exactly (FRAMING red, diff 1, total 3; INT_PIP green,
  diff 0, total 10).

No em dashes anywhere.
