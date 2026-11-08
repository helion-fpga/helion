# Brief: worker (device-file)

Role: worker. Worktree ~/workspace/helion-ultra-bitgen-device, branch
fm/ultra-bitgen-device, base fm/ultra-bitstream at aee77e3. You implement,
test, and report with evidence. You never message the user.

## Captain's intent

Work the branch we were working on until Helion's bitstreams are identical
to the reference, every last bit. Block by block testing, greedy approach.
You own the device files: xcku3p first (control), then xcvu3p.

Acceptance criteria: the pipeline goes green on xcku3p using the
BitFiltrator device file, and the xcvu3p file maps every FAR to a real
tile with every provisional value flagged with its source.

## CTO spec

- Read SPEC.md, INTERFACES.md, COORD.md first. Work in
  crates/helion-ultrascale (device.rs exists at base). Reference material:
  bitfiltrator-ref/resources/devices/xcku3p.json (complete, in the base
  worktree) and bitfiltrator-ref/src/bit_locator.py plus src/demo.py (the
  resolution flow: column major via clb_colMajors, Y_ofst = Y mod 60, tile
  type via clb_tileTypes).
- Deliverables, in order:
  1. A device-file loader for the BitFiltrator schema (clb_colMajors,
     clb_tileTypes, bram_content_colMajors, bram_reg_colMajors,
     max_far_row_idx, per-SLR IDCODE, config order) with tests that load
     xcku3p.json and resolve a known BEL (e.g. SLICE_X23Y45/B6LUT) to
     the exact (frame_offset, minor) pairs from ULTRASCALE_PLUS.json.
  2. A validation run of BitFiltrator demo.py reconstruction on xcku3p as
     the control: document the command, the output, and what it proves.
  3. A provisional xcvu3p.json built from xcku3p structure plus AMD
     XMP103 counts (394K LUTs, 788K FFs, 2280 DSP48E2, 720 RAMB36E2,
     320 URAM288). Every value not copied from a confirmed source is
     flagged `provisional: true` with its source. Nothing provisional is
     presented as confirmed.
- The xcvu3p file cannot be confirmed without Vivado (license blocked
  since Oct 22). Say so plainly in your report and in COORD.md. The
  confirmation job (create_device_summary.py, ~4 Vivado runs) stays a
  named blocker, not work you fake.
- Dependencies: bitgen-core (FAR assembly consumes your mapping),
  segbits-vendor (arch JSON pointers). Out of scope: bitgen emission
  (bitgen-core), per-BEL INIT wiring (lut-ff-bram), PIP emission
  (int-pip), the diff harness (diff-harness).
- Evidence required: loader tests passing, the xcku3p resolution example
  with hand-checked FAR arithmetic, demo.py output, and the xcvu3p file
  with its provisional flags listed. Report in COORD.md when done.
  Coordinate builds through the orchestrator (-j1/-j2).
- Standing rules: no em dashes anywhere. Never fabricate. A provisional
  value presented as confirmed is a FAIL.
