# Evening Push Summary (2026-10-14, 21:05 - 23:00 EDT)

## Commits (19 total)

### SERV QoR Track
1. `55fa9c1` - sv: use LutShare in seq/comb mapping paths
2. `4e079d7` - sv: fix LutShare netlist corruption (root cause: index_pin overwrite)
3. `d5b1355` - docs: AIG rewriting design (phases 1-4)
4. `d1c86d5` - sv: AIG cut enumeration for LUT mapping (Phase 1)
5. `2c799a3` - sv: AIG trivial cut detection (Phase 1b)
6. `7937a77` - sv: Document rewrite_trivial_cuts limitation

### Timing Track
7. `8c0e011` - docs: timing model calibration plan

### Backend Track (DSP/BRAM)
8. `3fb9e02` - docs: 7-series DSP/BRAM end-to-end wiring design
9. `b7435ad` - ir/sv: Define Dsp48e1 cell with pins, migrate synthesis
10. `747bd42` - xilinx7: PackedDsp carries opmode/alumode
11. `658eaaf` - xilinx7: PlacedDsp carries opmode/alumode
12. `7841a9d` - docs: DSP48E1 FASM emission research needed
13. `6185030` - ir: Define Bram18e1 cell with pins
14. `599591d` - xilinx7: PackedBram/PlacedBram carry width config
15. `0aee810` - sv/pack: Migrate BRAM synthesis to Bram18e1
16. `19f8e54` - ice40: Handle Dsp48e1/Bram18e1 in pack
17. `1607b86` - pack: Handle Dsp48e1 in HL10T pack

### Verification Track
18. `73e4b82` - docs: SAT-based equivalence checking design

## Test Status
- helion-sv: 226/226 pass
- MAC inference: 4/4 pass (with new Dsp48e1)
- BRAM: 6/6 pass (with new Bram18e1)
- Workspace: compiles clean

## Key Technical Decisions

### LutShare Bug (Fixed)
Root cause: Sharing LUTs with named outputs via `d.connect(o, existing_cell, "O")`
corrupted the netlist because `index_pin` uses HashMap::insert which OVERWRITES
the pin->net mapping, orphaning the original net. Fixed by reverting to safe
behavior (only share when out=None).

### Dsp48e1/Bram18e1 Cells
Replaced placeholder markers (Mac27/Bram18) with proper cells:
- Dsp48e1 { opmode: u8, alumode: u8 } with pins A[29:0], B[17:0], C[47:0], P[47:0]
- Bram18e1 { read_width_a/b, write_width_b } with full pin interface
- Wired through: IR -> synthesis -> pack -> place (xilinx7, ice40, ecp5, HL10T)
- FASM emission still needs Project X-Ray research

### AIG Cut Enumeration
Implemented k-cut enumeration with priority cuts (top 8 by size).
`rewrite_trivial_cuts` detects const/buffer/inverter but full rewriting
needs fanout tracking (future work).

## Blocked
- Push to wip/ide-integration: remote diverged, needs force-push authorization
- DSP FASM: needs Project X-Ray DSP48E1 bit mapping research
- Timing calibration: needs Vivado access / hardware

## Next Steps (by priority)
1. AIG rewriting full implementation (fanout tracking, node replacement)
2. DSP FASM emission (X-Ray research)
3. Timing-driven P&R (wire TimingGuide from Constraints)
4. SAT solver integration (kissat)
5. SVA/CDC, parallelism, new devices
