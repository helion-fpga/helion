# Brief: worker (integration)

Role: worker. Worktree ~/workspace/helion-ultra-bitgen-integ, branch
fm/ultra-bitgen-integ, base fm/ultra-bitstream at aee77e3. You implement,
test, and report with evidence. You never message the user.

## Captain's intent

Work the branch we were working on until Helion's bitstreams are identical
to the reference, every last bit. Block by block testing, greedy approach.
You own the end to end: `helion impl --part xcvu3p` producing a real
`.bit` file, and the gate that says whether it is identical.

Acceptance criteria: `helion impl` runs blinky and counter for xcku3p
and xcvu3p and emits a bitstream file; the diff harness runs on it; the
gate reports green or names the exact blocking block.

## CTO spec

- Read SPEC.md, INTERFACES.md, COORD.md first. Work in
  crates/helion-ultrascale and crates/helion-cli. Port the day-1 staging
  code into the crate (do not rewrite it): pack-place staging from
  fm/ultra-packplace c9e96c4 (pack.rs, place.rs, legality.rs), router
  scaffold from fm/ultra-router 3d49b98, timing staging from
  fm/ultra-timing 7ff0007. Note the day-1 SPEC correction already in the
  crate: one slice per CLB, one CARRY8 per slice. If a staging file
  contradicts the crate (e.g. two slices per CLB), the crate wins and you
  file the correction in COORD.md.
- Deliverables, in order:
  1. `helion impl --part xcku3p` and `--part xcvu3p` run synth, pack,
     place, route, STA, and bitgen on blinky and counter, and write a
     `.bit` file. Any stage that is not implemented for UltraScale+
     fails loud with the stage name; no silent fallback to another part,
     no empty bitstream file.
  2. Wire the seed through (PlaceOpts.seed exists; `helion impl --seed N`
     already exists from the multiseed worker). The diff harness golden
     runs use a fixed seed; record it.
  3. Own the gate: after every integration merge, run the diff harness
     on both designs and both parts and post the greedy queue to COORD.md.
     The gate is green only when every block is green or a named blocker.
- Dependencies: every other worker. You integrate in the orchestrator's
  order (largest red block first) and you do not start a stage until its
  owner has a green or blocked verdict from the critic.
- Out of scope: fixing block emitters yourself (route the fix to the
  owning worker); QoR work; any change to ice40, 7-series, ECP5, or HL10T
  behavior (prove unchanged with the existing suite on at least one
  design per family if you touch shared code; prefer not to touch it).
- Evidence required: the exact `helion impl` commands and their output,
  the emitted .bit files (sizes and first bytes), the diff-harness gate
  output after integration, and a cargo test run for the touched crates.
  Report in COORD.md when done. Coordinate builds through the
  orchestrator (-j1/-j2).
- Standing rules: no em dashes anywhere. Never fabricate. An empty or
  placeholder bitstream file presented as output is a FAIL.
