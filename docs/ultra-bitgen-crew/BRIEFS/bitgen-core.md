# Brief: worker (bitgen-core)

Role: worker. Worktree ~/workspace/helion-ultra-bitgen-core, branch
fm/ultra-bitgen-core, base fm/ultra-bitstream at aee77e3. You implement,
test, and report with evidence. You never message the user.

## Captain's intent

Work the branch we were working on until Helion's bitstreams are identical
to the reference, every last bit. Block by block testing, greedy approach.
You own the emitter: FASM in, bitstream out, and parse-back to identical
FASM.

Acceptance criteria: `emit_bitstream` and `parse_bitstream` round-trip
exactly for every FASM input; framing (sync word, FAR sequencing, CRC)
matches UG570; unknown features fail loud, never emit guessed bits.

## CTO spec

- Read SPEC.md, INTERFACES.md, COORD.md first. Work in
  crates/helion-ultrascale (exists at base: bel.rs, device.rs, fabric.rs,
  lib.rs). Add bitgen.rs (emitter + parser) plus tests. Do not touch
  pack/place/router/timing code; that belongs to the integration worker.
- Implement per INTERFACES.md: FASM parser for the format in the day-1
  report (`<tile>.<feature> [= <value>]`, `#` comments); segbits
  resolution through the SegbitsDb trait (segbits-vendor owns the trait and
  the data, you code against the trait shape in INTERFACES.md and a stub
  DB in tests until their data lands); frame assembly with the FAR layout
  (block type, row, column, minor; 93 words per frame); bitstream framing
  with sync word, FAR sequencing per config order, per-frame CRC per UG570.
- INIT expansion is MSB-first across the listed bits (bit 0 = LSB of the
  value). A `!` prefixed segbits token means the bit must read 0; assert
  it reads 0 in the base frameset and fail loud if it does not.
- Tests required: round-trip on a hand-built FASM file covering LUT INIT,
  FF INIT, one INT PIP, one BRAM INIT line (exact bytes asserted, computed
  by hand from the DB, not copied from your own output); sync word and CRC
  verified against UG570 values on a known frame; unknown feature returns
  BitgenError naming tile, feature, and reason; `!` bit set in base frames
  fails loud.
- Dependencies: segbits-vendor (trait + data), device-file (device file
  schema and FAR mapping). If their work is not ready, build against the
  INTERFACES.md trait shape with a stub and say so in your report.
- Out of scope: LUT/FF/BRAM per-BEL emission logic (lut-ff-bram worker),
  INT PIP emission logic (int-pip worker), the diff harness (diff-harness
  worker), end-to-end CLI wiring (integration worker).
- Evidence required: cargo test output for the crate, the exact commands,
  and the hand-computed expected bytes for at least one frame. Report in
  COORD.md when done. Coordinate builds through the orchestrator (-j1/-j2).
- Standing rules: no em dashes anywhere. Never fabricate. A fix without
  a regression test is not a fix.
