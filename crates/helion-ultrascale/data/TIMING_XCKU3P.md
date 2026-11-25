# xcku3p timing DB: inventory, sources, and gaps

Status: no real xcku3p STA delay DB exists yet. This file records exactly
what the ported STA code uses today, what public sources actually publish,
and what only Vivado can supply. Nothing here invents a delay.

## 1. What the ported STA code uses today (preliminary)

Location: `crates/helion-ultrascale/src/timing_db.rs`,
`preliminary_xcvu3p()`; consumed by `crates/helion-ultrascale/src/sta.rs`
(`sta_routed`) and by `crates/helion-cli/src/ultra.rs` (`sta_on_placed`).

| Field | Value | Basis |
|---|---|---|
| cells.lut6 | 120 ps, Ballpark | AMD forum ballpark, see below |
| cells.ff_ckq | 150 ps, Ballpark | same forum ballpark |
| cells.ff_setup | 80 ps, Ballpark | same forum ballpark |
| cells.ff_hold | 60 ps, Ballpark | same forum ballpark |
| carry8.ci_to_co_bit | 40 ps, Ballpark | same forum ballpark |
| carry8.s_to_o | 100 ps, Ballpark | same forum ballpark |
| carry8.cyinit_to_co | 60 ps, Ballpark | same forum ballpark |
| interconnect.single | 180 ps, Ballpark | same forum ballpark |
| interconnect.pip | 50 ps, Ballpark | same forum ballpark |
| dsp, bram, uram, iob, interconnect.double/quad/long | UNKNOWN | no source yet |

The ballpark source is an AMD community forum post giving roughly 0.3 ns
per logic level (LUT plus route combined) for UltraScale+. The port splits
that 300 ps into 120 ps LUT plus 180 ps one INT hop. The split itself is
an assumption, not a measured value: the forum post never splits LUT
from route. The other ballpark fields (flip-flop, CARRY8, PIP) are
likewise assumed allocations within that 0.3 ns level, and their source
strings say so; no individual value is forum-stated. Every number STA reports from this DB is an ESTIMATE, and
`TimingReport.is_estimate` plus the `estimate=1` token on the
`hang_diag timing` line say so. Critic session 10 flag F3 is exactly
this: `sta_on_placed` took no part argument and always used
`preliminary_xcvu3p()`, so the xcku3p WNS carried no xcku3p-specific
delay data. The fix in this workstream (landing on branch
fm/ultra-bitgen-timing): `sta_on_placed` takes the part, selects the
DB through `sta_db_for_part()`, and the report is labeled estimate
until a real DB can run STA.

## 2. What a real xcku3p delay DB would need to contain

For `sta_routed` to run at all, these four fields must be present
(anything else UNKNOWN fails loud with `timing: ... UNKNOWN`):

- cells.lut6: LUT6 input-to-output delay, per input pin if possible
  (Xilinx notes the I5 input is faster than the others).
- cells.ff_ckq: flip-flop clock-to-Q.
- cells.ff_setup: flip-flop setup requirement (hold is checked
  separately when hold analysis lands).
- interconnect.single: one INT tile hop, plus a per-hop PIP cost and
  the double/quad/long segment delays for longer routes. The current
  STA models route delay as Manhattan distance times `single`, which
  is itself an approximation even with a real per-hop number.

For full coverage the DB also needs carry8 (CI to CO per bit, S/DI to
O, CYINIT to CO), DSP48E2 (A/B to P multiply-only, CLK to P),
RAMB36E2 (CLK to DOUT, address setup), URAM288 (CLK to DOUT, address
setup), and IOB (pad to fabric, fabric to pad). Those fields exist in
`UltraScaleTimingDb` as `Option` values; UNKNOWN means missing, never
zero.

## 3. What public sources actually publish (checked 2026-11-19)

### DS922, Kintex UltraScale+ Data Sheet: DC and AC Switching Characteristics

This is the correct datasheet for xcku3p (Kintex UltraScale+). Column
mapping for every table below: the five numeric columns are -3 at
0.90V, -2 at 0.85V, -1 at 0.85V, -2 at 0.72V, -1 at 0.72V. The DB uses
speed grade -2 at 0.85V, which is the second numeric column, not the
fourth: an earlier reading took the fourth column (1.46 ns) for -2 at
0.85V, but that column is -2 at 0.72V.

- Block RAM and FIFO Switching Characteristics (DS922 Table 33,
  docs.amd.com/r/en-US/ds922-kintex-ultrascale-plus/Block-RAM-and-FIFO-Switching-Characteristics):
  TRCKO_DO (CLK to DOUT, without output register) at -2 / 0.85V is
  1.02 ns = 1020 ps, Max. TRCKO_DO_REG (with output register) at the
  same grade is 0.29 ns = 290 ps. FMAX_WF_NC 738 MHz, FMAX_RF 637 MHz,
  TPW 542 ps at the same grade. (An earlier reading quoted the
  fourth-column values 0.42 ns, 585 MHz, 510 MHz, 577 ps here; those
  are the -2 at 0.72V values.) This is the ONLY pin-to-pin delay in
  DS922 that maps onto a field of `UltraScaleTimingDb`, and it is the
  only documented value in `documented_xcku3p()` /
  `documented_xcvu3p()`: `bram.clk_to_do` = 1020 ps, source
  "DS922 Table 33 TRCKO_DO, -2 0.85V". (1.46 ns is the TRCKO_DO value
  at -2 / 0.72V, the fourth numeric column; it is not the -2 / 0.85V
  value.) The struct has no separate
  registered-output field, so TRCKO_DO_REG is recorded here only, not
  stored. No address-setup row exists in the BRAM table, so
  `bram.addr_setup` stays UNKNOWN.
- UltraRAM Switching Characteristics (DS922, same column mapping):
  only FMAX values (FMAX 600 MHz, FMAX_ECC_NOPIPELINE 400 MHz,
  FMAX_NOPIPELINE 500 MHz at -2 / 0.85V) and TPW 700 ps. (An earlier
  reading quoted 500 MHz, 312 MHz, 404 MHz, 800 ps; those are the -2
  at 0.72V values.) No
  clock-to-output delay is published. 1/FMAX is a period, not a
  pin delay, so it must not be stored as `uram.clk_to_do`. Both URAM
  fields stay UNKNOWN.
- DSP48 Slice Switching Characteristics (DS922): only FMAX values
  (FMAX 775 MHz with all registers used, FMAX_NOPIPELINEREG 410 MHz
  at -2 / 0.85V). (An earlier reading quoted 644 MHz and 323 MHz;
  those are the -2 at 0.72V values.) No pin-to-pin delay is published. Both DSP fields
  stay UNKNOWN.
- FPGA Logic Switching Characteristics (DS922): this section covers
  IOB HD/HP tables only (TINBUF_DELAY_PAD_I, TOUTBUF_DELAY_O_PAD,
  TOUTBUF_DELAY_TD_PAD), with values per I/O standard. There is no
  CLB LUT or flip-flop delay table, and no standard-independent
  scalar that could fill `iob.pad_to_fabric` or
  `iob.fabric_to_pad`. Both IOB fields stay UNKNOWN.
- No DS922 table publishes a LUT6 delay, a flip-flop clock-to-Q or
  setup/hold, a CARRY8 delay, or any interconnect wire/PIP delay.
  The Xilinx forum answer on LUT interior switching states the
  internal LUT design is proprietary and only the external delay
  from LUT inputs to the slice register is observable inside Vivado.

### DS923, Virtex UltraScale+ Data Sheet (xcvu3p)

Same BRAM numbers as DS922 at -2 / 0.85V: TRCKO_DO 1.02 ns,
TRCKO_DO_REG 0.29 ns (docs.amd.com/r/en-US/ds923-virtex-ultrascale-plus/Block-RAM-and-FIFO-Switching-Characteristics).
`documented_xcvu3p()` therefore carries the same single documented
value, `bram.clk_to_do` = 1020 ps. (1.46 ns is the TRCKO_DO value at
-2 / 0.72V, not at -2 / 0.85V.) All other fields UNKNOWN, for the
same reasons as DS922.

### UG574, UltraScale Architecture CLB User Guide

Structure only (slice contents, CARRY8, wide muxes). No delay values.

### BitFiltrator arch data (ULTRASCALE_PLUS.json)

Location tables only: LutLoc, RegLoc, BramMemLoc, BramMemParityLoc.
No delay field exists anywhere in the file. It cannot supply timing.

### prjuray-db segbits

Bit encodings only. No delays.

## 4. Conclusion for STA

A real xcku3p DB that can run `sta_routed` does NOT exist from public
sources: the four required fields (lut6, ff_ckq, ff_setup,
interconnect.single) are all unpublished. `documented_xcku3p()` is
therefore deliberately unusable for STA: calling `sta_routed` on it
fails loud with `timing: LUT6 delay UNKNOWN in TimingDb`. Until the
Vivado data below lands, STA runs on the preliminary ballpark DB and
every report is labeled estimate. That labeling, plus the fail-loud
documented DB, is the F3 fix: no number is ever presented as real
timing while its source is a forum ballpark.

## 5. Unblocker: what only Vivado can supply, and how

License state is UNKNOWN (license server 27000@license2.forest.usf.edu
connection refused since Oct 22, 2026; no check run from this VM).
When a licensed Vivado is available, on the cluster, run on the
placed and routed blinky and counter for part xcku3p-2 (speed grade
-2), with a 10 ns clock constraint:

```
vivado -mode batch -source extract_delays.tcl
```

where `extract_delays.tcl` does, after `place_design` and
`route_design` on each design:

- `report_timing -delay_type max -max_paths 20 -path_type full -file timing_full.rpt`
  (per-segment logic and net delays for the worst paths; the logic
  delays on LUT/FF/CARRY8 segments are the per-cell numbers the DB
  needs, and the net delays per hop are the interconnect numbers).
- `report_timing_summary -file timing_summary.rpt` (WNS/TNS ground
  truth for the same placements, to validate Helion STA against).
- `write_sdf -file design.sdf` (Standard Delay Format: per-cell
  IOPATH delays and per-net INTERCONNECT delays, machine-readable,
  the preferred source for filling the DB).
- For the cell library itself (not just two designs):
  `report_property [get_timing_arcs -of_objects [get_cells -hierarchical -filter {PRIMITIVE_TYPE =~ CLB.*}]]`
  is not a delay source; the authoritative per-arc delays come from
  the SDF plus `report_timing -path_type full` segment breakdowns
  across a sweep of small probe designs (single LUT, LUT chain,
  FF to FF at several Manhattan distances, CARRY8 chain, DSP, BRAM,
  URAM). Each probe gives one DB entry; the probe RTL and the
  Vivado version must be recorded with the numbers.

Do not use RapidWright .dat files or FPGA Interchange timing data:
both are under the Xilinx EULA and are rejected by the crew spec.
Do not derive pin delays from FMAX values (a period is not a delay).
