# QoR size status (Muse)

Branch: `fm/qor-size`. Scope: Helion-xilinx7 up to packer cell counts only.
Never timing, place, or route code: that is DeepSeek's territory. Heavy
builds, tests, and sweeps run on the USF Slurm cluster only; this VM is
unit tests only. Guards on every change: x7v verify and svcheck.

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
| G126 | Small multiplies (8x8 and below) go to DSP; Vivado uses LUTs. Copy Vivado's thresholds. | In progress (this session) |
| G125 | VHDL multiply on DSP; reuse the G3 path. | Open |
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
- Explicit `use_dsp = "yes"` still forces DSP. Mux-wrapped (clear/enable)
  small multiplies, sums of products, `a * b - c`, constant-operand
  multiplies, and ranged/bit operands stay on DSP for now; those are
  residuals for G128.
- Open check items for the cluster size run: the 9x9 boundary (if Vivado
  uses a DSP for 9x9, the constant moves from 10 to 9) and asymmetric
  multiplies (rule uses the wider operand).
- Tests: 13 new G126 unit tests in `crates/helion-sv/src/lib.rs` plus
  `dependent_param_ow_equals_two_times_dw` widened from DW 8 to DW 16 so
  it still exercises the DSP path. All 13 pass. Full helion-sv lib suite:
  254 tests, 252 pass; the 2 failures are pre-existing on the base
  (`dp_lut_var_index_write_is_hffs_not_skip_assign`,
  `refuse_inline_bit_select_on_non_ident_arg`, both multiply-free) and
  the same holds for the integration suites (`ug901_infer` 5/7,
  `bus_soft` 3/4, `sv2017_sysfuncs_packages` 3/4, all pre-existing).
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
- Pre-existing findings (not changed by G126, recorded for follow-up):
  - A combinational `assign p = a * b + c` is dropped by the front end
    before synthesis (flatten_cap: nested arith in an assign), so only
    the clocked-NBA MAC form reaches synthesis.
  - A mux-wrapped (clear/enable) MAC flattens to one NBA per output
    bit and each per-bit NBA infers its own full DSP (16 DSPs for a
    16-bit MAC); the DSP path also drops the mux conditions.
  - helion-sim cannot simulate a DSP multiply whose operands are
    whole-bus nets: its Mul model reads operand bits `a_0`, `a_1`, ...
    but `emit_mac27` connects the whole bus `a`, so the bus reads as
    one bit. That is why the DSP-path sim value tests were not kept.
