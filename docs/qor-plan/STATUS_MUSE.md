# QoR size status (Muse)

Branch: `fm/qor-size-4.5` (cut from origin/fm/release-4.5 at ce268e05;
G126 cherry-picked onto it at 0399a4c7). The earlier `fm/qor-size`
branch (master-based, G126 at 333098ce) is frozen; never force-push it.
Scope: Helion-xilinx7 up to packer cell counts only. Never timing,
place, or route code: that is DeepSeek's territory. Heavy builds,
tests, and sweeps run on the USF Slurm cluster only; this VM is unit
tests only. Guards on every change: x7v verify and svcheck.

There is no HELION_GAPS file in the repo (checked on this branch and
on master); the gap record lives here.

## Targets

- Corpus LUT ratio vs Vivado: median 1.02x or less, p90 1.30x or less,
  total within 5 percent, no design over 2x.
- DSP, BRAM, carry, and FF counts equal to Vivado on every micro unit.
- PicoRV32 and Ibex at most 1.2x Vivado LUTs.
- Every Ibex unit packs in 8 GB.

## Today (Nov 27, 2026)

- Corpus LUT ratio: median 1.09x, p90 3.00x, total plus 15 percent.
- PicoRV32: 1.67x Vivado LUTs. Ibex: 1.69x Vivado LUTs.

## Gaps, in order

| Gap | What | Status |
|-----|------|--------|
| G126 | Small multiplies (8x8 and below) go to DSP; Vivado uses LUTs. Copy Vivado's thresholds. | Landed on fm/qor-size-4.5 at 0399a4c7 (cherry-pick of 333098ce) |
| G125 | VHDL multiply on DSP; reuse the G3 path. | Landed via the coverage lane (see below) |
| Coverage | The 10-bit decision must cover every multiply form, not just bare a*b / a*b+c over plain idents. | Landed (this lane, hashes in the report) |
| G127 | Popcount and hamming as LUT trees. | Open |
| G130 | Ibex over 62 GB; ibex_multdiv_fast mac_res_signed_6 huge cone; remove unobservable logic first, multipliers to DSP. | Open |
| G128 | Rest of the LUT tail, one cause at a time. | Open |
| CPUs | PicoRV32 and Ibex at most 1.2x Vivado LUTs. | Open |

## G126 status

- Vivado threshold established from UG902 v2020.1 Table 15, p.170:
  multiplications which are >= 10 bits are implemented on a DSP48 macro
  cell; multiplication lower than this limit are implemented using LUTs.
  Rule used: DSP iff the wider operand is at least 10 bits, so 8x8 and
  9x9 map to LUTs and 10x10 stays on DSP.
- Decision point: `crates/helion-sv/src/ug901.rs`, step 1 of
  `apply_ug901_dsp_pass`, which runs before the `emit_mac27` paths.
  Intercepted, only when no explicit `use_dsp` is set: exact bare
  `a * b` and exact single `a * b + c` / `c + a * b`, all operands plain
  idents, output width within the product (and `c`) width.
- Lowering: a new Carry4 array multiplier in the same file, one LUT per
  partial-product bit (S LUT folds the AND into the carry-chain S input,
  Carry4 carries). An 8x8 multiply is 64 LUTs plus 14 Carry4 cells,
  matching Vivado's 61-71 LUTs for an 8x8 LUT multiplier; the LUT6-only
  array multiplier needs 128 LUTs. A clocked MAC registers the sum once,
  so it stays a single register stage like the DSP PREG it replaces.
- Explicit `use_dsp = "yes"` still forces DSP.
- Open check items for the cluster size run: the 9x9 boundary (if Vivado
  uses a DSP for 9x9, the constant moves from 10 to 9) and asymmetric
  multiplies (rule uses the wider operand).
- Tests: 13 new G126 unit tests in `crates/helion-sv/src/lib.rs` plus
  `dependent_param_ow_equals_two_times_dw` widened from DW 8 to DW 16 so
  it still exercises the DSP path. All 13 pass. Full helion-sv lib suite
  on the 4.5 base: 265 passed, 0 failed.
- Guards (exact numbers in the G126 worker report):
  - helion-sim value guard (the effective guard): 4 sim tests over 75
    deterministic vectors each (11 corners plus 64 xorshift) pass for
    comb 8x8, reg 8x8, reg MAC 8x8, and comb 9x9. The LUT multiplier
    computes the exact unsigned product and the MAC keeps one register
    stage.
  - svcheck (rung5 script) cannot verify multiply designs as configured:
    it models a Carry4 carry-out only on a pin literally named `CO`,
    while Helion emits per-bit `CO0`-`CO3`, so it ties every Carry4
    carry chain to zero (the pre-existing 32-bit adder fails svcheck
    the same way); and it does not model the `Dsp48e1` cell kind at all,
    so the DSP path fails there too. Both are pre-existing svcheck
    limitations, not G126 regressions. The helion-sim tests above are
    the guard that actually covers the changed code.
  - x7v verify: not feasible on this VM (no Project X-Ray zynq7
    database at `~/.cache/helion/prjxray-db`, no zedbit binary, no
    `nvc`, no Vivado golden bitstream for a multiply design). The
    bitstream-level check belongs on the cluster run.

## Coverage status (this lane)

G126 only fired on a bare `a*b` or `a*b+c` with plain ident operands.
Every other form still went to a DSP whatever its width. The coverage
lane moves the decision to one place and covers every form.

- Single decision point: `mul_uses_dsp(wa, wb)` in
  `crates/helion-sv/src/ug901.rs` (DSP iff the wider elaborated operand
  is at least 10 bits, `VIVADO_DSP_MIN_BITS = 10`). It is consulted by
  the whole-RHS matcher (`match_whole_mul_form`, which matches bare
  `a*b`, `a*b+c`, `a*b-c`, and `a0*b0+a1*b1` with generalized operands)
  and by the embedded-multiply splice (`splice_small_muls`), and the
  flatten-cap exemption consults the matcher through
  `whole_mul_form_eligible` in the same file. There is no second
  matcher: VHDL multiplies flow through the same `synth_rtl_inner`
  and DSP pass (helion-vhdl translates VHDL to SV, then calls
  `helion_sv::synth_sv_sources`), so the G125 VHDL routing is this
  same decision, not a separate path.
- Operand widths are elaborated widths, resolved by
  `mul_operand_width`: Ident is the signal width, Range is the span,
  Bit is 1, Const is the declared width, Concat is the sum of the part
  widths. Bit nets come from `mul_operand_bits` (via the existing
  `extract_arith_operand_nets`, plus per-part resolution for Concat).
- Forms now covered (each with exact counts plus a helion-sim value
  test): slice operands (`a[7:0]*b`, `a[15:8]*b`, 64 LUTs / 14 Carry4 /
  0 DSP), bit operand (`a[3]*b`, 8 LUTs / 7 Carry4 / 0 DSP), constant
  operands (`a * 8'd5`, 72 LUTs / 14 Carry4 / 0 DSP), concat operands
  (`{a[3:0], b[3:0]} * c`, 64 LUTs / 14 Carry4 / 0 DSP),
  enable/clear-mux-wrapped multiplies (one shared 8x8 product, muxes
  kept, 291 LUTs / 14 Carry4 / 16 HFFs / 0 DSP; see the LUT note below),
  `a*b-c` (combinational and registered, 80 LUTs / 18 Carry4 / 0 DSP,
  Carry4 subtract with S = XNOR and CYINIT = 1), and sums of two
  products (combinational and clocked, 144 LUTs / 32 Carry4 / 0 DSP).
  Combinational MAC/sub/sumprod reach the pass only because the
  flatten-cap exemption now covers whole-RHS multiply forms; the cap
  path itself also lowers small whole-RHS forms to LUTs instead of
  dropping the assign or emitting a DSP per NBA.
- The splice (`splice_small_muls`, step 1c of the DSP pass) replaces
  each small multiply embedded in a larger RHS (mux arms, wider Add
  trees, per-bit NBAs from if/else flattening) with an Ident of a
  shared temp product. Each unique multiply is emitted once per module
  and shared: the enable-wrapped MAC flattens to 16 per-bit NBAs that
  all contain the same Mul, and one temp product serves all 16
  (verified by the cell breakdown: exactly one 64-LUT multiplier).
  Wide multiplies stay in place for the DSP steps; IndexPart/WordAt
  subtrees are never entered (address math is not a DSP multiply).
- Constant-operand decision: the constant's DECLARED width is used
  (`a * 8'd5` goes to LUTs, `a * 16'd5` stays on DSP). Evidence: the
  repo's Vivado data does NOT settle this. `tools/qor/vivado.json`
  (Vivado 2017.3, xc7z020clg484-1, 2026-11-17) holds only blinky,
  bram_fill, dsp_fill_prims, ibex, mmcm, and pico; there is no multiply
  micro unit in it, and `tools/qor/baseline.json` holds the same 12
  run.py designs with no micro units either. So the choice follows the
  UG902 width rule (a constant is an operand whose width is its
  declared width) and is flagged here as needing cluster confirmation:
  if Vivado puts a small-valued wide constant on LUTs (or folds it by
  shift-add), the constant rule must be revisited. A constant operand
  materialized as Lut6 const cells costs one LUT per constant bit on
  top of the multiplier (8 for `8'd5`); shift-add const folding is a
  follow-up, not this lane.
- Enable-wrapped LUT note: 291 LUTs is heavy against Vivado's ~80 for
  the same form. The breakdown is one shared 8x8 product (64 LUTs,
  14 Carry4) plus the per-bit bit-blast of the 16-bit add (172 LUTs, no
  Carry4: the if/else flattens to per-bit NBAs, so the add never reaches
  the Carry4 ripple helper) and the clear/enable muxes (53 LUTs). The
  decision itself is right (one shared product, 0 DSPs) and the values
  are correct (sim proves clear/hold/accumulate); the whole-vector
  reconstruction that would route the add through Carry4 is G128
  scope, not a decision-coverage gap.
- Tests: 22 new coverage tests in `crates/helion-sv/src/lib.rs`
  (counts plus helion-sim value checks, 75 vectors each where a sim
  test applies) plus 11 ported G125 VHDL tests in
  `crates/helion-vhdl/tests/g125_vhdl_dsp.rs` (counts plus sim values;
  the G125 worker was stopped before pushing, so its test file and
  probe findings were ported here verbatim, with a header note). Two
  pre-existing tests were updated, not weakened: the 8-bit ranged
  comb-multiply DSP pin (`comb_mul_assign_infers_mac27`) moved to
  DW=16 because an 8x8 multiply belongs on LUTs under the approved
  rule (its ranged form is pinned to LUTs by the coverage slice test),
  and the G126 mux-wrapped DSP pin (`g126_mux_wrapped_mac_8x8_stays_dsp`)
  was renamed and re-pinned to the new LUT lowering. The shared
  `g126_bit_blast_ports` test helper now expands a port into per-bit
  ports for every bit net that exists, not just a contiguous run from
  bit 0, so sliced operands can be driven in sim.
- Full helion-sv lib suite on the coverage tip: 287 passed, 0 failed
  (265 base + 22 new). Full helion-sv suite (all targets): 429 passed,
  0 failed across 22 targets (407 on the rebase baseline + 22 new).
  helion-vhdl: 60 passed, 0 failed (36 lib + 11 ported G125 tests + 13
  across the other targets).
- Guards (exact):
  - helion-sim value tests are the effective guard: 22 coverage tests
    include sim value checks (75 deterministic vectors each where a
    sim test applies) plus the 11 G125 VHDL tests include 3 sim value
    tests; all pass. The LUT multiplier, the Carry4 subtractor, and
    the sum-of-products form compute the exact unsigned results.
  - svcheck: BLOCKED on this VM, not passed. The `hnf` release binary
    cannot be built from this branch: `helion-cli` unconditionally
    depends on `helion-ultrascale`, which fails to compile with a
    pre-existing E0061 (`write_frame_bit` called with 9 arguments at
    `crates/helion-ultrascale/src/bitgen.rs:1761`, takes 3 at
    `bitgen.rs:1616`; the file is unmodified vs HEAD 0399a4c7, so the
    error is on the base, and it is UltraScale+ bitgen code, out of
    this lane's scope). Verilator 5.053 is available (oss-cad-suite),
    so the hnf build is the only blocker, and no prebuilt hnf exists
    in any worktree. The expected svcheck failure mode for multiply
    designs was verified by reading `tools/svcheck/svcheck.py`:
    its Carry4 model only emits a carry-out for a pin literally named
    `CO` (line 327), while Helion emits per-bit `CO0`-`CO3`, and its
    cell dispatch has no branch for `Dsp48e1` or `Mac27` (lines
    230-328), so both paths are unmodelled there. Both are
    pre-existing svcheck limitations, not coverage regressions.
  - x7v verify: not feasible on this VM (no Project X-Ray zynq7
    database at `~/.cache/helion/prjxray-db`, no zedbit binary, no
    `nvc`, no Vivado golden bitstream for a multiply design). The
    bitstream-level check belongs on the cluster run.
- Unit list for the cluster check: `docs/qor-plan/sub10bit-units.md`.
  No run.py design (12 designs) contains a multiply with both operands
  under 10 bits (verified by source inspection: Ibex's multiplies are
  17x17, dsp_fill's are 25x18 and 32-bit, pico's multiplier is disabled,
  the rest have no multiply), so the run.py check is a regression gate
  (counts unchanged). The 309-design sweep list lives in DeepSeek's
  cluster harness, not the repo; DeepSeek runs exactly the sub-10-bit
  units from his harness and reports before/after LUT and DSP counts
  vs Vivado per unit.

## Residuals (pre-existing, not changed by this lane)

- A wide `a*b-c` still goes to `emit_mac27`, which wires P = A*B and
  drops the subtraction (`accum_c` only matches Add). Pinned by
  `cov_large_sub_16x16_stays_dsp` (routing only, values deliberately
  not asserted). Fixing the wide sub is G128 scope.
- A `c - a*b` form (product as subtrahend) is not matched by the
  whole-RHS matcher; the splice still lowers its multiply, and the
  surrounding Sub bit-blasts. Correct, just not Carry4-fast.
- In a capped module, embedded (mux-wrapped) multiplies are not
  spliced, so they keep the DSP path there; only whole-RHS forms lower
  to LUTs in the cap path.
- `use_dsp = "no"` / `"logic"` on a MAC, sub, or sum of products is
  still ignored by the main NBA loop (`emit_mac27` has no mode check
  there); the explicit-attribute path only lowers a bare multiply.
  Pre-existing, out of scope.
- A combinational `assign p = a * b + c` is no longer dropped: the
  flatten-cap exemption now covers whole-RHS multiply forms, so it
  reaches the DSP pass. (This closes the G126 pre-existing finding
  that it was dropped.)
- A mux-wrapped (clear/enable) MAC no longer infers 16 DSPs and no
  longer drops the mux conditions: the splice lowers the one shared
  small multiply to LUTs and keeps the muxes. (This closes the G126
  pre-existing finding.)
- helion-sim cannot simulate a DSP multiply whose operands are
  whole-bus nets: its Mul model reads operand bits `a_0`, `a_1`, ...
  but `emit_mac27` connects the whole bus `a`, so the bus reads as
  one bit. That is why the DSP-path sim value tests were not kept.
