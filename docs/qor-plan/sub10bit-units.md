# Sub-10-bit multiply units (coverage lane, fm/qor-size-4.5)

Date: Nov 28, 2026. The coverage lane moves the DSP-or-LUT decision to
one place (`mul_uses_dsp` in `crates/helion-sv/src/ug901.rs`, DSP iff the
wider elaborated operand is at least 10 bits) and covers every multiply
form. This file lists which sweep units that decision can change.

## The 309-design sweep list is not in this repo

There is no 309-design sweep list in the repo or the workspace: no file
under `tools/qor/` names 309 units, and `tools/helion_vs_vivado.py`
does not exist on any branch (verified on master and release-4.5). The
309 list lives in DeepSeek's cluster-side harness. DeepSeek must map the
sub-10-bit units from his own harness; this file gives him the
repo-visible analysis and the real `run.py` invocation for the
regression check below. Do not invent a 309-unit list from this file.

## Repo-visible analysis: the 12 run.py designs

`tools/qor/run.py` DESIGNS (the ZedBoard QoR benchmark, not the
309-design sweep) were inspected by source for a multiply operator
(`*`) and the elaborated width of each operand. Result: **none of the
12 designs contains a multiply with both operands under 10 bits**, so
the coverage change must change **no** run.py design's cell counts.
The run.py check for this change is a regression gate (counts
unchanged), not a before/after.

| Design | Multiply in source | Operand widths | Verdict |
|--------|-------------------|----------------|---------|
| blinky | none | - | unaffected |
| counter | none | - | unaffected |
| complex | none | - | unaffected |
| hier_bitsel | none | - | unaffected |
| pico | none instantiated: `ENABLE_MUL (0)`, `ENABLE_FAST_MUL (0)` in `examples/zedboard_pico_soc.sv`. The uninstantiated `picorv32_pcpi_fast_mul` in `third_party/picorv32/picorv32.v` is 32x32. | 32x32 (uninstantiated) | unaffected |
| bram_fill | none | - | unaffected |
| dsp_fill_prims | explicit DSP48E1 instances; LFSR hash multiplies `h = (l+1) * 32'h9E3779B1`, `h = h * 32'h85EBCA6B` | 32-bit | unaffected (DSP) |
| dsp_fill | MAC `a <= S[24:0]; b <= S[31:14]; m <= a * b; acc <= acc + m` (25-bit by 18-bit); LFSR hash multiply 32-bit | 25x18, 32-bit | unaffected (DSP) |
| ibex | `ibex_multdiv_fast`: `mult1/2/3_res = $signed({sign, 16-bit}) * $signed({sign, 16-bit})` (3x); `gen_mult_fast`: `mac_res_signed = $signed({sign_a, mult_op_a}) * $signed({sign_b, mult_op_b}) + accum`, `mult_op_a/b` 16-bit | 17x17 (all four) | unaffected (DSP) |
| mmcm | none | - | unaffected |
| ibex_soc | same Ibex core as `ibex` | 17x17 | unaffected (DSP) |
| bram_fill_25mhz | none | - | unaffected |

Notes:

- The Ibex multiplies are 17 bits wide because each operand is a
  16-bit half plus a sign bit (`{sign, op[15:0]}`); 17 is above the
  10-bit threshold, so they stay on DSP. The G130 item (the
  `mac_res_signed` sum-of-three cone) is about the adder tree, not
  about these multiplies being sub-10-bit.
- Vivado's own ibex build uses 1 DSP (`tools/qor/vivado.json`,
  `ref_ibex`, dsp 1) against Helion's 3 (one per 17x17 multiplier);
  that sharing gap is separate from this lane and is not closed here.
- The `k*2` / `k*4` terms in `examples/ysyx_ibex.sv` are generate-loop
  index arithmetic in part-selects, not datapath multiplies.

## Real invocation for DeepSeek (cluster)

Regression check on the run.py designs (counts must be unchanged;
`run.py` gates against `baseline.json` and exits 1 on a regression):

```sh
HELION_XILINX_DB=~/.cache/helion/prjxray-db python3 tools/qor/run.py --set quick --json qor-coverage.json
```

The multiply-bearing subset, if the full set is too slow (Ibex routing
can take hours; `--timeout` bounds each build):

```sh
HELION_XILINX_DB=~/.cache/helion/prjxray-db python3 tools/qor/run.py --only pico,dsp_fill,dsp_fill_prims,ibex,ibex_soc --json qor-coverage-subset.json
```

`run.py` builds the `zedbit` release binary itself, then each design
for `xc7z020clg484-1`; logs and bitstreams go to `<target dir>/qor/`.
`--set quick` is blinky, counter, complex, hier_bitsel, pico;
`--set full` adds bram_fill, dsp_fill_prims, dsp_fill, ibex, mmcm,
ibex_soc, bram_fill_25mhz. Do not use `--accept` on this run unless
a count moved for a reason this lane documents; the expectation is
zero movement.

## The 309-design check (DeepSeek's harness)

Run exactly the sweep units in DeepSeek's harness whose RTL contains a
multiply with both operands under 10 bits (slice, constant, concat,
bit, enable-wrapped, `a*b-c`, sum of products, VHDL or SV), on the
coverage tip, and report before/after LUT and DSP counts vs Vivado per
unit. The micro units the coverage tests pin (8x8 slice, 8x8 constant,
8x8 concat, 1x8 bit, enable-wrapped 8x8 MAC, 8x8 `a*b-c`, 8x8 sum of
products, and their VHDL equivalents) are the forms to look for; the
repo cannot name the 309 units, so the per-unit list must come from
the harness side.
