# ZB-04: CARRY4 inference (helion-sv must create Carry4 cells)

- Track: ZedBoard, frontend plus backend. Investigator finding C2
  (emit side). The emit side is dead code until this lands.
- Branch: fm/zedboard-fix-carry4, tip d2d067a (finding only, no
  implementation). Worktree: ~/workspace/helion-zed-bringup.
- Status: stage 1 done (Nov 8, 2026, commit 9f90549 on
  fm/zedboard-fix-router, not fm/zedboard-fix-carry4). Opt-in
  `synth_sv_carry` lowers a single whole-register `lhs <= lhs + 1`
  to Carry4 cells plus S-LUTs and rewired FFs; helion-sim models
  the chain; 3 sim tests prove equivalence to the cone flow.
  Backend carry-through (pack pin nets, place, route, emit) open.

## Goal

Helion's SystemVerilog frontend must infer CARRY4 cells from adder and
counter logic, and the backend must carry their configuration (DI, S,
CYINIT) all the way to the bitstream.

## Done when

Test t03 (t03_carry4_counter) passes on the board: the counter counts
correctly, which proves a real carry chain works end to end. Also: a
design with a 27-bit counter must produce Carry4 cells in the IR (grep
Carry4 in the lowered netlist returns hits), and the emitted FASM must
contain PRECYINIT plus the ACY0/BCY0/CCY0/DCY0 mux bits for those cells.

## What is already done

- Finding only. The CARRY4 worker on Oct 22, 2026 proved the emit code is
  dead code: helion-sv never creates Carry4 cells (grep Carry4 in
  helion-sv returns 0 hits; the IR has a comment saying inference is
  future work). The blinky's 27-bit counter is built from LUTs, not carry
  chains. CARRY4 is NOT the blinky bug.
- The emit side was changed to emit valid ACY0/BCY0/CCY0/DCY0 mux bits
  instead of the invalid `CARRY4.IN_USE` feature (which does not exist in
  the X-Ray DB). That is future-proofing, not a fix, because the code
  never executes.
- Investigator C2 documents exactly what is missing: PRECYINIT (CYINIT
  source select: AX / CIN / C0 / C1), DI input muxing, S input
  configuration, and inter-CARRY4 chaining (CI from the previous CO). The
  PlacedCarry struct (place.rs:86) only has name / tile / site, no
  configuration data. The pack stage (pack.rs:103) captures no DI/S/CYINIT
  info from the IR.

## What was tried and what failed

- Nothing implemented yet. The only attempt was the emit-side cleanup
  above, which changed dead code.

## What to do

1. In helion-sv, add adder inference: recognise addition and increment
   patterns in the lowered IR and emit Carry4 cells. Start with the
   27-bit counter pattern from examples/zedboard_blinky.sv. The QoR
   campaign has the same need (simpleuart's residual is ripple adder logic
   with no Carry4 adder inference; see US-10). Share the approach, do not
   build it twice.
2. Extend PlacedCarry (place.rs:86) with DI, S, and CYINIT configuration
   data. Extend the pack stage (pack.rs:103) to capture DI/S/CYINIT from
   the IR.
3. In emit.rs, emit PRECYINIT (AX / CIN / C0 / C1) plus the ACY0/BCY0/
   CCY0/DCY0 mux bits, from the captured configuration. Never emit
   `CARRY4.IN_USE`; it does not exist in the DB.
4. Verify in stages: first, the lowered IR contains Carry4 cells for the
   counter; second, the emitted FASM contains the right features; third,
   t03 passes on the board. Each stage must pass before the next starts.
5. Coordinate with ZB-08 (CARRY4 vertical adjacency). Inference without
   adjacency still breaks the chain; adjacency without inference has
   nothing to place.

## Files and commands

- Frontend: crates/helion-sv (search for the Carry4 comment and the
  adder lowering).
- Backend: crates/helion-xilinx7/src/pack.rs (line 103),
  src/place.rs (PlacedCarry, line 86), src/emit.rs (CARRY4 emit,
  investigator cites lines 194-201).
- DB: ~/workspace/xilinx-dbsrc, segbits_clbll_l.db (PRECYINIT.AX/CIN/C0/C1,
  CARRY4.ACY0/BCY0/CCY0/DCY0).
- Test: ~/workspace/zedboard-hw-loop/tests/t03_carry4_counter.sv.
- Golden decode script:
  docs/zedboard-handoff/findings/chain_golden.py (reads GT-blinky.bit
  via work/x7decode.py in the Mac handoff package, resolves features
  via segbits_clblm_r.db). Full output:
  docs/zedboard-handoff/findings/blinky_chain_golden.txt (Nov 8,
  2026).

## Golden chain (decoded Nov 8, 2026 from GT-blinky.bit, verified v2)

Method warning that bit the first decode: CLBLM tiles share 36
frames, 2 words per tile. Any decode that sweeps whole frames per
tile smears neighbors (it invented 46 FFs and FFMUX.CY bits).
The v2 decoder restricts to the tile's own words and requires
full segbits term sets; scripts archived in
docs/zedboard-handoff/findings/ (chain_golden.py, carry_census.py,
blinky_chain_golden.txt). Device-wide census: exactly 12 chained
halves, all in X71Y39-Y45 (utilization reports 13 CARRY4, off by
one, unexplained, recorded below).

Vivado's 25-bit counter plus registered LD0 is exactly 26 FFs:

- Y39-Y43: SLICEM_X0 holds 4 FFs each (bits 0-23); SLICEL_X1
  holds NO FFs. Y44: SLICEM_X0 4 FFs plus SLICEL_X1 DFF (bit
  24). Y45: SLICEM_X0 AFF only (LD0). Total 20+5+1 = 26.
- Every used FF: ZINI+ZRST set, FFMUX.XOR set (only XOR, never
  CY), FFSYNC set (FFSYNC is a ZB-03 gap: unimplemented).
- Every chain LUT: INIT 0xffffffff00000000, except entry:
  SLICEL_X1.ALUT 0xff00ff0000000000, SLICEM_X0.ALUT
  0x0f0f0f0f0f0f0f0f (Y39).
- PRECYINIT: SLICEL_X1 AX at Y39 then CIN Y40-Y44; SLICEM_X0
  CIN Y40-Y45. M0(Y39) and both halves of Y38/Y46 read C0, which
  is the all-clear default (no chain there).
- Only one CARRY4 mux bit in the repo: ACY0 at the AX entry.
  No INIT_OUT, CEUSEDMUX, SRUSEDMUX anywhere.

Open questions (do not guess; trace before implementing):

- L1(Y39-Y43) role: chained carry plus 4 LUTs each, zero FFs.
  S inputs are intra-slice (no S site pins exist), so no S/DI
  routing is needed, but what the L1 carry computes is unknown.
- 12 chained halves vs 13 reported CARRY4s: one unaccounted.
  Y39 M0 (default C0) is the suspect; verify before relying.
- Bit-to-BEL map inside M0 halves needs S/DI/Q net tracing.
- FFMUX.XOR-only means FF D comes from the XORCY sum output,
  not carry CO and not fabric LUT O. The D connection is
  intra-slice (no routing, no S/DI routing either: S has no site
  pins at all). The only carry nets that can leave a slice are
  AX (entry CYINIT source, routable via BYP pips), CIN/COUT
  (cascade), DI (single site pin), and Q fanout. Net-level
  consequence: a carry group needs zero inter-slice routing
  except AX/DI feed and the CO cascade.
- L1(Y39-Y43) carry with zero FFs: its LUTs/chain serve the M0
  computation in the same CLB (S/DI assist). Pack must keep
  M0+L1 of a chain CLB together; exact bit roles need net
  tracing but reproduction needs only the recipe below.

## Reproduction recipe (what the backend must emit per group)

Per 4-bit group (middle groups; entry differs): 1 Carry4 cell
with S0-S3 nets from 4 LUT O6 outputs (each LUT INIT
0xffffffff00000000, i.e. O6=Q on A5 and O5=0), DI0-DI3 from the
same LUTs' O5 (=0) or const-0, CI chained from previous CO
(PRECYINIT.CIN, no mux bits), 4 FFs with D on the carry O nets
(DFFMUX.XOR), ZINI+ZRST, FFSYNC. Entry group: CYINIT=1 via
PRECYINIT.AX (AX from a 1-source), ACY0 set, ALUT INITs
0xff00ff0000000000 (L1) and 0x0f0f0f0f0f0f0f0f (M0).
Top partial group as Y44/Y45. IR pin convention is documented
on CellKind::Carry4 (CI, CO, S0-S3, DI0-DI3, O0-O3); entry CI
takes a const-1 net and the backend picks AX vs CIN.

## Interception point (helion-sv)

Per-bit cones are built around map_wide_cones (u_wshare_ trees),
so recognition must happen BEFORE per-bit cone splitting, on the
whole update `reg[N:0] <= reg + const`. helion-sim exists as a
crate: any inference must prove equivalence by simulating the
original RTL against the inferred netlist before it ships
(a wrong Carry4 is worse than none). Detection stays opt-in
until the backend consumes the cells end to end.

## Dependencies

Depends on nothing for the frontend work. The hardware proof depends on
ZB-01 (clock) and ZB-08 (adjacency). Related to US-10 (QoR Carry4 adder
inference is the same problem at a different scale).

## Rules

- No em dashes anywhere.
- Never add a fix primarily to make failing tests pass without verifying
  the synthesis semantics. A Carry4 that is inferred wrong is worse than
  no Carry4.
- The blinky does not need CARRY4 to blink. Do not let this block ZB-01.
