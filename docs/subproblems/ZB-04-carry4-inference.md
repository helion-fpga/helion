# ZB-04: CARRY4 inference (helion-sv must create Carry4 cells)

- Track: ZedBoard, frontend plus backend. Investigator finding C2
  (emit side). The emit side is dead code until this lands.
- Branch: fm/zedboard-fix-carry4, tip d2d067a (finding only, no
  implementation). Worktree: ~/workspace/helion-zed-bringup.
- Status: not started. This is new implementation work, not a fix to
  existing code.

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

## Golden chain (decoded Nov 8, 2026 from GT-blinky.bit)

Vivado's 25-bit counter plus registered LD0 (26 flops) in
CLBLM_R_X71, rows Y39 to Y45. Every used FF: ZINI+ZRST set,
FFMUX.CY and FFMUX.XOR both set, FFSYNC set (FFSYNC is a ZB-03
gap: unimplemented). Every chain LUT: INIT 0xffffffff00000000,
except the entry slice (below).

- Y39 entry: SLICEL_X1 PRECYINIT.AX + CARRY4.ACY0; ALUT
  0xff00ff0000000000, BLUT/CLUT/DLUT 0xffffffff00000000.
  SLICEM_X0 ALUT 0x0f0f0f0f0f0f0f0f, other LUTs
  0xffffffff00000000; 4 FFs, FFSYNC, no PRECYINIT, no CARRY4 mux
  bit on M0.
- Y40-Y43: both halves PRECYINIT.CIN, all LUTs
  0xffffffff00000000, 8 FFs each, FFSYNC on M0.
- Y44: SLICEL_X1 PRECYINIT.CIN with DFF only (bit 24);
  SLICEM_X0 PRECYINIT.CIN with 4 FFs; FFSYNC both halves.
- Y45: SLICEM_X0 AFF only (the LD0 output FF), PRECYINIT.CIN,
  FFSYNC, ALUT 0xffffffff00000000, AFFMUX.CY set (D fed from
  carry, not fabric).
- Chained cells need no ACY0/BCY0/CCY0/DCY0 (only the AX entry
  sets ACY0); no INIT_OUT, CEUSEDMUX, SRUSEDMUX anywhere.

Open questions (do not guess; trace before implementing):

- M0(Y39) carry source: 4 FFs with FFMUX.CY but no PRECYINIT
  and no CARRY4 mux bit. Needs net-level tracing (which nets
  drive its S/DI) before choosing the entry model.
- FFMUX.CY+XOR both set on every FF: reproduce both bits
  exactly; mux semantics unverified.
- LD0 AFF D source: AFFMUX.CY implies carry-O feed; confirm
  against INT routing before wiring pack output.
- Bit-to-BEL map (which counter bit sits where) needs the
  S/DI/Q net tracing, not just config bits.

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
