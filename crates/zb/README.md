# crates/zb: the front ends as proven on the ZedBoard

These crates are unchanged copies of four crates from the branch that first
put Helion bitstreams on a ZedBoard (`fm/zedboard-verify`, commit `f4af1a5`):

| crate here | copy of | what it is |
|---|---|---|
| `helion-ir-zb` | `helion-ir` | the netlist IR of that time (plain string names), with the LUT re-mapper |
| `helion-sv-zb` | `helion-sv` | the first SystemVerilog front end, with its hold, initializer and width fixes, carry-chain adders, subtractors, comparators and the adder-tree multiplier |
| `helion-vhdl-zb` | `helion-vhdl` | the VHDL front end (`v2`): VHDL to flat SystemVerilog, then `helion-sv-zb` |
| `helion-sim-zb` | `helion-sim` | the netlist simulator with the carry-chain model |

`helion-zb-front` is new: it calls them and converts the resulting netlist
to the current IR (`helion_zb_front::synth_sv`, `synth_sv_carry`,
`synth_vhdl_top`, `vhdl_to_sv`).

## Why copies

Both branches changed the first SystemVerilog front end after they split,
and they fixed the same problems in different ways (what a register holds
when a branch does not assign it, how adders are lowered, how output pads
are created). A line-by-line merge of the two would have produced a front
end that neither branch had tested. Master's copy stays as it is in
`crates/helion-sv`; this is the other one, exactly as it was verified.

The 7-series backend (`crates/helion-xilinx7`) and the bitstream verifier
(`tools/x7v`) are merged for real and are not copied here.

## What uses them

* `crates/helion-xilinx7/examples/zedbit.rs` with `--front=zb`, and as the
  fallback of `--front=auto` when the v2 front end refuses a design.
* VHDL in `zedbit`: the translation to SystemVerilog always comes from
  `helion-vhdl-zb`; the SystemVerilog is then synthesized by the v2 front
  end, or by `helion-sv-zb` under `--front=zb`.
* The ZedBoard tests of `helion-xilinx7` (`blinky_reg_e2e`, `zedboard_e2e`,
  `ff_place_dump`), which compare against Vivado's bitstream for designs
  synthesized this way.
* `tools/x7v synthcheck` and `svcheck` (through the `synth2v` and `vhdl2sv`
  examples of `helion-vhdl-zb`).

Their unit tests are switched off here: they place and route on fabric
crates of their own time. They run in the branch the copies come from.

## When they can go

When the v2 front end (`crates/helion-sv/src/v2`) infers carry chains and
reads VHDL, nothing above needs these crates and they can be deleted.
