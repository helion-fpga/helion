# Brief: worker (int-pip)

Role: worker. Worktree ~/workspace/helion-ultra-bitgen-int, branch
fm/ultra-bitgen-int, base fm/ultra-bitstream at aee77e3. You implement,
test, and report with evidence. You never message the user.

## Captain's intent

Work the branch we were working on until Helion's bitstreams are identical
to the reference, every last bit. Block by block testing, greedy approach.
You own the INT_PIP block: every routed wire in the bitstream, bit exact.

Acceptance criteria: for a routed design, every INT PIP Helion emits
matches the prjuray-db segbits entry exactly, including the `!` bits
that must read 0. Diff zero on INT_PIP, critic re-run.

## CTO spec

- Read SPEC.md, INTERFACES.md, COORD.md first. Work in
  crates/helion-ultrascale. Source of truth: segbits_int.db (3778 lines,
  FASM format `INT.PIP.<src>.<dst> <bit> !<bit> ...`) via the SegbitsDb
  trait (segbits-vendor owns it). Router input shape: UltraRouted from
  the day-1 router scaffold (fm/ultra-router 3d49b98) and the WireDb seam
  in the day-1 crew INTERFACES.md.
- Deliverables, in order:
  1. PIP emission: given a routed net (driver wire to sink wires through
     PIPs), emit the FASM lines that set exactly the segbits for each
     PIP, including asserting every `!` bit reads 0 in the base frameset.
     Test: the BOUNCE_E_0_FT1 example from the day-1 report resolves to
     its exact bits (64_00 set, 66_00, 68_00, 72_00, 72_01, 74_00 zero).
  2. A routed-design test: route blinky (or counter) through the router
     scaffold's mock WireDb, emit PIPs, and verify every emitted bit
     against the DB by lookup, not by re-reading your own output.
  3. A missing-PIP report: any PIP the router needs that has no segbits
     entry goes in a list with tile, src, dst, and the design that needed
     it. Missing PIPs fail loud at emit time; they are never skipped
     silently and never guessed.
- The 3778-entry DB may not cover every mux in an INT tile (completeness
  unverified, day-1 report). Your missing-PIP list IS the input to the
  completeness audit; keep it exact.
- Dependencies: segbits-vendor (INT DB + trait), device-file (INT tile
  X/Y), bitgen-core (emitter), diff-harness (INT_PIP block status).
  Out of scope: LUT/FF/BRAM emission, IO/clock/DSP/URAM, the diff
  harness itself, end-to-end CLI wiring.
- Evidence required: per-PIP tests passing with exact commands, the
  BOUNCE example hand-checked, the routed-design verification, and the
  missing-PIP list (empty or exact). Report in COORD.md when done.
  Coordinate builds through the orchestrator (-j1/-j2).
- Standing rules: no em dashes anywhere. Never fabricate. A guessed PIP
  encoding is a FAIL.
