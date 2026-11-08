# Brief: worker (lut-ff-bram)

Role: worker. Worktree ~/workspace/helion-ultra-bitgen-lut, branch
fm/ultra-bitgen-lut, base fm/ultra-bitstream at aee77e3. You implement,
test, and report with evidence. You never message the user.

## Captain's intent

Work the branch we were working on until Helion's bitstreams are identical
to the reference, every last bit. Block by block testing, greedy approach.
You own the first three blocks: LUT INIT, FF INIT/config, BRAM content
and parity. They are the biggest blocks; greedy says they go first.

Acceptance criteria: for a placed design, every LUT INIT bit, every FF
INIT bit, and every BRAM content and parity bit Helion emits matches the
BitFiltrator tables exactly. Diff zero on all three blocks, critic re-run.

## CTO spec

- Read SPEC.md, INTERFACES.md, COORD.md first. Work in
  crates/helion-ultrascale. Source of truth:
  bitfiltrator-ref/resources/archs/ULTRASCALE_PLUS.json: LutLoc (64 frame
  offsets per LUT, A6LUT through H6LUT, Y_ofst 0..59), RegLoc (16 FF names
  A..H FF and FF2), BramMemLoc (16384 content offsets, Y_ofst 0..23),
  BramMemParityLoc (2048 offsets). LUTRAM uses the same INIT bits as LUTs
  (paper section V-B).
- Deliverables, in order:
  1. LUT emission: given a placed LUT (slice X/Y, LUT letter, 64-bit
     INIT), emit the FASM lines (or call bitgen-core's emitter) that set
     exactly the 64 bits from LutLoc. Test: a LUT with a known INIT
     (e.g. XOR pattern 64'h6996966996696996) produces exactly the 64
     (frame, bit) pairs from the JSON, checked by hand on at least one
     bit and by table lookup on all 64.
  2. FF emission: same treatment for FF INIT and config via RegLoc
     (16 FFs per slice). Test with INIT 0 and INIT 1.
  3. BRAM emission: content (16384 bits) and parity (2048 bits) via
     BramMemLoc and BramMemParityLoc. Test with a small known content
     pattern; verify the first and last bits by hand against the JSON.
- Consume the placed design from pack-place staging shapes (SLICE_X/Y,
  BEL names A6LUT-H6LUT, AQ-HQ). Do not rewrite the packer; read its
  Placed output shape from the day-1 crew INTERFACES.md.
- Dependencies: segbits-vendor (arch JSON access), device-file (slice
  X/Y to tile resolution), bitgen-core (emitter). Out of scope: INT PIP
  emission, IO/clock/DSP/URAM, the diff harness, end-to-end CLI wiring.
- Evidence required: per-block tests passing with exact commands, one
  hand-checked bit per table, and the diff-harness block status for
  LUT_INIT, FF_INIT, BRAM_CONTENT, BRAM_PARITY on blinky and counter.
  Report in COORD.md when done. Coordinate builds through the
  orchestrator (-j1/-j2).
- Standing rules: no em dashes anywhere. Never fabricate. A golden file
  copied from your own output instead of the DB is a FAIL.
