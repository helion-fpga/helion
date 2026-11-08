# Brief: worker (diff-harness)

Role: worker. Worktree ~/workspace/helion-ultra-bitgen-diff, branch
fm/ultra-bitgen-diff, base fm/ultra-bitstream at aee77e3. You implement,
test, and report with evidence. You never message the user.

## Captain's intent

Work the branch we were working on until Helion's bitstreams are identical
to the reference, every last bit. Block by block testing, greedy approach.
You own the scoreboard: the harness that says, per block, how far from
identical we are, largest diff first. Without it the greedy loop has no
queue.

Acceptance criteria: one command runs the full diff on blinky and counter
for xcku3p and xcvu3p, prints blocks ordered by diff size descending,
and records placement hash plus seed with every run.

## CTO spec

- Read SPEC.md, INTERFACES.md, COORD.md first. The harness lives outside
  the repo at ~/workspace/ultra-bitgen-crew/harness/ (same precedent as
  the QoR harness in ~/workspace/qor-campaign/): scripts plus golden
  data. In-repo test additions go on your branch.
- Deliverables, in order:
  1. Golden FASM for blinky and counter: placed and routed once, frozen,
     with the placement hash and seed recorded. Placement and bitstream
     options are held constant across runs; a run whose placement hash
     differs from the golden is rejected, not compared.
  2. The diff itself: for each block in the registry (LUT_INIT, FF_INIT,
     BRAM_CONTENT, BRAM_PARITY, INT_PIP, IO_CONFIG, CLK, DSP48E2,
     URAM288, FRAMING), compute diff_bits against the tier-1 reference
     (segbits DBs via the SegbitsDb trait plus BitFiltrator tables) and
     emit the JSONL record from INTERFACES.md with status green, red, or
     blocked (blocked names the exact missing data in a `missing` field).
  3. The greedy queue: print blocks ordered by diff_bits descending.
     That ordering is the crew's work queue; do not reorder it by hand.
  4. Round-trip check: parse_bitstream(emit_bitstream(f)) == f exactly
     for every golden FASM. A mismatch is a FRAMING red, not a warning.
- Tier-2 slot: the harness accepts a `--reference-bit` flag for a
  Vivado bitstream. With no reference bit present it reports tier 1
  only and says so. It never invents a reference.
- Dependencies: bitgen-core (emitter + parser), segbits-vendor (DBs),
  device-file (device files), lut-ff-bram and int-pip (their blocks'
  emitters). Until an emitter lands, its block reports `blocked` with
  the missing piece named, never zero.
- Evidence required: the harness running end to end on both designs and
  both parts, the printed greedy queue, and one hand-verified diff
  record (recompute a block's diff_bits by hand on a small case).
  Report in COORD.md when done. Coordinate builds through the
  orchestrator (-j1/-j2).
- Standing rules: no em dashes anywhere. Never fabricate. A harness that
  reports green on a comparison it did not run is a FAIL.
