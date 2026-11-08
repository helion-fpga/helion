# Brief: worker (segbits-vendor)

Role: worker. Worktree ~/workspace/helion-ultra-bitgen-segbits, branch
fm/ultra-bitgen-segbits, base fm/ultra-bitstream at aee77e3. You implement,
test, and report with evidence. You never message the user.

## Captain's intent

Work the branch we were working on until Helion's bitstreams are identical
to the reference, every last bit. Block by block testing, greedy approach.
You own the ground truth: the segbits databases, vendored legally, loaded
through one API.

Acceptance criteria: every segbits lookup in the pipeline resolves from
vendored repo data (never /tmp), with license verified, and the INT PIP
completeness audit has a number.

## CTO spec

- Read SPEC.md, INTERFACES.md, COORD.md first. The /tmp/prjuray-db clone
  from day 1 is gone (deployments wipe /tmp). Re-clone
  github.com/f4pga/prjuray-db fresh. Verify its CC0 1.0 license file in
  the clone before vendoring anything. Verify the BitFiltrator MIT LICENSE
  in bitfiltrator-ref/ before vendoring its JSON pointers.
- Deliverables, in order:
  1. Vendor the needed DBs under crates/helion-ultrascale/data/: the
     zynqusp segbits files the pipeline needs (segbits_int.db with 3778
     INT PIP lines, segbits_hpio_right.db, segbits_hdio_top_right.db,
     segbits_hdio_bot_right.db, segbits_cmt_right.db, plus the site-type
     JSONs the audit needs). Each vendored file carries a header comment
     naming its source repo, commit, and license.
  2. Implement the SegbitsDb trait from INTERFACES.md (lookup by tile
     type and feature, tile_types list). Loader reads only repo data.
     Tests: lookup of a known INT PIP line (e.g. the BOUNCE_E_0_FT1
     example from the day-1 report) returns the exact bits from the file;
     unknown feature returns None; `!` tokens parse to must_be_zero.
  3. INT PIP completeness audit: enumerate the PIPs of one INT tile from
     the site-type JSONs and from the PinPlanning probe output when it
     exists, and report covered vs missing counts against the 3778-entry
     DB. If the probe output does not exist yet, say so and give the
     audit in two parts: DB-internal count now, probe comparison pending.
     Never present an unaudited count as complete.
- Legal line (hard): RapidWright .dat files and FPGA Interchange DBs are
  REJECTED (Xilinx EULA). Never ingest them or data derived from them.
  If you find a file whose provenance you cannot verify, leave it out and
  flag it in COORD.md.
- Dependencies: bitgen-core consumes your trait; int-pip consumes the INT
  DB; device-file consumes the arch JSON pointers. Out of scope: bitgen
  emission, frame addressing, per-BEL wiring, the diff harness.
- Evidence required: clone commit hashes, license verification output,
  loader tests passing, audit counts with exact commands. Report in
  COORD.md when done. Coordinate builds through the orchestrator (-j1/-j2).
- Standing rules: no em dashes anywhere. Never fabricate. Unverified
  provenance is a FAIL.
