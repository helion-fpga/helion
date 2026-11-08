# UltraScale+ bitgen handoff: everything needed to resume from here

State as of Nov 8, 2026, about 11:45 EST. Branch `fm/ultra-bitstream`, tip `060b230`
(merge of the tile-type-dependent INT column offset, `a63f49c`). Pushed to
`github.com/helion-fpga/helion` on that branch. Nothing merged to master.
Nothing else pushed from this campaign before today.

## Why this file exists

Focus shifted to the ZedBoard on Nov 8, 2026, because real hardware can verify
it. UltraScale+ is NOT abandoned: it continues in parallel through the hourly
driver cron `bitgen-100-driver` and the critic session in flight (see below).
This file plus the crew docs in this directory plus the harness in
`crates/helion-ultrascale/harness/` are the complete resume kit. A new person
should be able to start from here without re-analyzing the project.

## What is proven (tier 1: zero diff vs public segbits DBs)

Base tip `060b230`: `cargo test -j2 -p helion-ultrascale` gives 153 passed,
0 failed. Harness run `runs/srval-complete/diff.jsonl` (20 records) is the
reference result. On xcku3p, blinky and counter, all critic re-run:

- LUT_INIT green: blinky 0/64, counter 0/256
- FF_INIT green: blinky 0/2, counter 0/8 (INIT plus SRVAL lines)
- INT_PIP green: blinky 0/11, counter 0/17
- FRAMING green: 0/3 on both (formal, critic session 15 PASS, no defects)

## Block status

- IO_CONFIG blocked, 55 bits, standing largest blocked item. No xcku3p
  tilegrid exists in any public source. New flag from Nov 8: the device file
  lists HPIO_L, not HPIO_RIGHT (HPIO_RIGHT occurs 0 times in the BitFiltrator
  files), so the golden's HPIO tile type may not exist on xcku3p at all. If so,
  IO_CONFIG needs a new HPIO_L fuzzing campaign, not just a tilegrid.
  Unblocker: the Q6 Vivado probe (see below).
- BRAM_CONTENT, BRAM_PARITY blocked: no BRAM golden design exists yet. The
  whole-array INIT to per-word INIT translation is still unwritten.
- CLK, DSP48E2, URAM288 blocked: no encodings exist. They need Vivado
  fuzzing, which is license-blocked.
- LUTRAM blocked: no public LUTRAM table exists.
- CARRY8: ruled its own harness block Nov 8 (CTO ruling, COORD.md). Never
  fold CARRY8 bits into LUT_INIT. The registry implementation is still pending.
- xcvu3p provisional only: computed frame total 73,035 vs UG570 Table 1
  71,820. The emitter does not wire it up. Never tune the file to close the
  gap; it needs Vivado confirmation.
- Only 2 designs have ever been diffed. The random-design generator is not
  built. The 100-design loop has not started.
- Tier 2 (zero diff vs a real Vivado bitstream) has never happened. Zero real
  Vivado diffs exist. Never claim Vivado-equivalence without one.

## The gaps, all of them (from the Nov 8 CTO list)

Bitgen pipeline: IO_CONFIG (above); BRAM golden missing; DSP/URAM/CLK
encodings missing; LUTRAM blocked; CARRY8 block unimplemented; xcvu3p
provisional; 2 designs only; tier 2 never run; preamble registers Vivado
writes (TIMER, WBSTAR, COR0, COR1, others) deliberately NOT emitted because
their UltraScale+ values are unknown in any open source; ending mirrored from
7-series; frame-0 CRC state derived by linear algebra, not observed; last-of-row
2-zero-padding path never exercised (0 of 261 FARs); a valued feature whose
value is all zero is indistinguishable from absent on parse-back.

No end-to-end flow: the pipeline takes FASM in, not Verilog. Helion impl fails
loud at ROUTE (WireDb empty), the router-to-PIP translator is not written,
pack/place are staging code (one slice per CLB, one CARRY8 per slice), timing
analysis is preliminary with no real xcku3p delay data (sta uses
preliminary_xcvu3p for both parts).

Netlist quality: honest QoR scoreboard is 23 red and 13 green vs Yosys on 36
designs, target is Vivado, which sits above Yosys. The K=4 mapper has a proven
miscompile and its branch repair is pending; no Carry4 adder inference for
procedural incrementers; BRAM inference incomplete; picorv32 frontend loop and
cv32e40p gaps open.

Device coverage: one part, xcku3p, plus provisional xcvu3p. No SSI support.
No path to a new part without a device file we do not have.

Vivado the product: no XDC constraints, no IP cores, no clocking (MMCM/PLL),
no DRCs, no timing closure loop, no debug cores, no power analysis, no partial
reconfiguration, no real timing DB.

Validation and speed: no UltraScale+ hardware in this loop, nothing proven on
silicon. The harness takes about 9 minutes for 2 designs (bits walk over full
16.5 MB bitstreams, one bits run took 111 s). The 100-design loop needs a
speedup. Plain `cargo clippy --all-targets` is blocked on a pre-existing
helion-sv error at lib.rs:15059 (overly_complex_bool_expr), not this crew's
files.

## In flight right now (Nov 8, 2026)

- Critic session 16: re-reviewing commit `a63f49c` (INT column offset fix)
  plus the harness-side `reference.py` change. The INT fix is formal only
  after this PASS. Worktree `~/workspace/helion-ultra-bitgen-tilegrid` kept
  at `a63f49c` for the critic. Verdict lands in CRITIQUE.md.
- Driver cron `bitgen-100-driver`: hourly, goal
  virtex-ultrascale-backend-support. Works the queue until 100 random designs
  are green at tier 1 AND tier 2. Logs at
  `~/workspace/goals/virtex-ultrascale-backend-support/hidden_files/bitgen-100-driver/run-log.md`.
  Delivery to the side chat only on a block going green, a critic FAIL, a new
  blocker, every 10-design tier-1 milestone, tier-1 100/100, or tier-2
  progress. Quiet runs stay quiet.
- Queue order after critic 16: BRAM golden design, then DSP/URAM/CLK coverage
  as encodings allow, then the random-design generator plus the 100-design
  harness loop, then tier 2 when the license returns.

## License and the Q6 probe

- License server 27000@license2.forest.usf.edu: connection refused Oct 22
  through Nov 7. On Nov 8 about 10:27-10:31 EST the port ANSWERED (driver run
  twice, direct TCP check once). A real license checkout is still UNVERIFIED:
  no lmutil or Vivado exists on this VM. The real test is Vivado on the
  cluster (sc.rc.usf.edu, campus/VPN only) or the PinPlanning probe.
- The Q6 Vivado probe is the tilegrid unblocker. It is written up in
  `BRIEFS/tilegrid-research.md` (1,173 lines): step 1 tile inventory
  (extract_device_info.tcl plus `TILE_TYPE =~ HPIO*` arbiter queries, which
  alone settles HPIO_RIGHT vs HPIO_L), step 2 one row-0 HPIO tile base FAR
  plus frame count, step 3 the offset ruling. It cannot run from this VM.
- Vivado fuzzing package (jobs 1-5, 30 files) is at
  `~/workspace/your_files/vivado-fuzzing-instructions.md` and
  `vivado-fuzzing-package.zip`. The PinPlanning probe was prepared but never
  run on the cluster.
- Sandbox limits (do not re-discover): outbound SSH from this VM is blocked
  at the network level; sc.rc.usf.edu is unreachable from here (campus/VPN
  only); USF VPN needs interactive Microsoft SAML SSO plus Duo, it cannot be
  completed headlessly. Practical route: prepare scripts locally, Saksham runs
  them on the cluster, pastes back outputs.

## Values that are provisional or invented (never treat as truth)

- HPIO bridge: `HpioProvisionalFallback` in
  `crates/helion-ultrascale/src/bin/helion-bitgen.rs` maps HPIO_RIGHT_X0Y0 to
  base FAR 0x000. The 0x000 is INVENTED, pinned by test so any change is
  deliberate. It dies the day the xcku3p tilegrid lands. Literal deletion
  regresses every green block to blocked (critic session 14 reproduced this).
  Do NOT delete it until the tilegrid actually resolves HPIO tiles.
- INT column rule: CLEL_R serves INT at column -1, the other three CLB types
  at column +1 (commit `a63f49c`). Correct per the minors-table check, but
  absolute-provisional until the tilegrid lands. Critic 16 re-review pending.
- Device file `data/xcku3p.json` frame total 41,460 vs UG570 Table 1's 41,476
  for KU3P (gap 16, exactly 4 per FAR row, systematic). Header LUT/FF counts
  are Vivado part-property truth, never derived from geometry. xcku3p adopts
  AMD's 20,340 slices (162,720 LUTs, 325,440 FFs); the file's num_slices
  27,120 contradicts its own num_luts and is a file-header anomaly.
  xcvu3p adopts 49,260 slices (394,080 LUTs, 788,160 FFs); device.rs 49,250
  is a rounding artifact and must be corrected if touched.
- `device.rs` grid is INVENTED (144x400, hard-block columns every 12,
  (x+y)%3 SLICEM ratio). Counts are right but it is NOT ground truth. The
  bitgen code must never consume it for FAR mapping.
- BRAM tile: one RAMB36E2 OR two RAMB18E2 halves, never both (type-level
  `BramTileUse`). RAMB18 halves are named _L / _U (lower/upper), never _A/_B.
- FF INIT polarity (resolved, do not re-derive naively): INIT value 0 selects
  .V0, which SETS the bit; INIT value 1 selects .V1, which CLEARS the bit.
  Bit = NOT(INIT). SRVAL the same. SRVAL bit derives from the INIT bit by a
  within-slice delta: -1 for A/C/E/G letters, +1 for B/D/F/H letters.
- Canonical FASM for round-trip checks is COMPLETE: every FF in the design
  gets an INIT line and an SRVAL line, default .V0 (Xilinx defaults are 0).
  The emitter does not invent defaults; the harness canonicalizes.
- The preamble registers Vivado writes are deliberately NOT emitted (values
  unknown for UltraScale+). Guessed values are never emitted.

## Branch map (all local, all pushed Nov 8, 2026)

- `fm/ultra-bitstream` (tip 060b230): the base. Everything merged lives here.
- `fm/ultra-device` (7a4146f): helion-ultrascale crate origin.
- `fm/ultra-bitgen-core` (5e26453): bitgen.rs, FASM parser, frame assembly,
  writer/parser, CRC-32C, reconciled SegbitsDb/BitRef.
- `fm/ultra-bitgen-device` (33a4b8f): device file loader, xcku3p.json,
  provisional xcvu3p.json, demo.py reconstruction validation.
- `fm/ultra-bitgen-segbits` (c1bef40): vendored prjuray-db segbits DBs plus
  LUT/FF/BRAM tables, PROVENANCE.md, INT PIP audit part 1.
- `fm/ultra-bitgen-lut` (0e81099): bitloc.rs emission tables.
- `fm/ultra-bitgen-lutff` (7f80b54): canonical .V0/.V1 FF INIT/SRVAL.
- `fm/ultra-bitgen-int` (6bb30b6): INT PIP emission, 3778 PIPs.
- `fm/ultra-bitgen-diff` (aee77e3): diff harness base (harness itself lives
  outside the repo, see below).
- `fm/ultra-bitgen-integ` (f5ebcf7): helion impl wiring, WireDb seam, fasm_gen.
- `fm/ultra-bitgen-bitgendev` (4fd7120): BitgenDevice impl for DeviceFile.
- `fm/ultra-bitgen-emitter` (9290ce6): helion-bitgen executable.
- `fm/ultra-bitgen-tilegrid` (a63f49c): INT column offset fix, critic 16 scope.
- Day-1 branches (fm/ultra-bitstream base history): fm/ultra-packplace,
  fm/ultra-router, fm/ultra-timing, fm/ultra-multiseed, fm/ultra-qor,
  fm/ultra-routing. Staging only; the integration port superseded them.

## File map

In this repo on `fm/ultra-bitstream`:
- `crates/helion-ultrascale/` : the crate. `src/bitgen.rs` (emit/parse),
  `src/segbits.rs` (SegbitsDb trait, BitRef), `src/bitloc.rs` (LUT/FF/BRAM
  bit locations), `src/device_file.rs` (device loader), `src/bitgen_device.rs`
  (BitgenDevice impl), `src/int_pip.rs`, `src/fasm_gen.rs`,
  `src/bin/helion-bitgen.rs` (emitter executable, HPIO bridge lives here),
  `data/` (vendored xcku3p.json, ULTRASCALE_PLUS.json, segbits DBs, LUT/FF/BRAM
  tables, PROVENANCE.md), `harness/` (diff harness scripts, tests, goldens).
- `docs/ultra-bitgen-crew/` : this handoff, SPEC.md, INTERFACES.md, COORD.md,
  CRITIQUE.md, BRIEFS/ (all worker briefs plus tilegrid-research.md),
  gate.sh (integration gate 2 script).

Outside the repo (local only, NOT pushed; 350 MB, mostly harness runs):
- `~/workspace/ultra-bitgen-crew/` : the live crew dir. COORD.md is the
  running log (3,198 lines), CRITIQUE.md the critic verdicts (2,875 lines).
  `harness/` holds the live harness; `harness/runs/` holds every harness run
  (317 MB of 16.5 MB bitstreams, excluded from the repo copy on purpose).
  `harness/reference/` (33 MB: BitFiltrator JSON, xcku3p.json, prjuray-db
  clone) is also excluded; its vendored equivalents are in `data/`.
- `~/workspace/prjuray-db-ref` : prjuray-db reference clone, commit affbc5e,
  CC0 1.0. `~/workspace/prjuray-tools-ref` : prjuray tools clone (read-only,
  Apache-2.0, never vendor from it without a CTO ruling).
- `~/workspace/helion-ultra-bitstream/` and `helion-ultra-bitgen-*` :
  worktrees, one per branch above.

## How to resume (verify first, then work the queue)

1. `cd ~/workspace/helion-ultra-bitstream && git log --oneline -1` (expect
   060b230) and `git status --short` (expect only `?? bitfiltrator-ref/`).
2. `cargo test -j2 -p helion-ultrascale` (expect 153 passed, 0 failed).
3. `cd ~/workspace/ultra-bitgen-crew/harness && python3 diff.py` (expect
   exit 0; compare `runs/<latest>/diff.jsonl` against
   `runs/srval-complete/diff.jsonl`: expect zero differing records).
4. Read the tail of `~/workspace/ultra-bitgen-crew/COORD.md` for the latest
   queue state, and `CRITIQUE.md` for the latest critic verdict.
5. Work the queue in order (see "In flight" above). Greedy rule: diff harness
   orders blocks by diff size descending, fix largest first, one block per
   fix, one fix per commit, re-run the full diff after each fix, revert on
   the spot any fix that regresses a green block, a block goes green only on
   zero diff PLUS a critic re-run.
6. Crew rules (`~/workspace/custom-skills/crew-flow/SKILL.md`): one spawner
   per crew; CTO spawns orchestrator plus critics only, the orchestrator
   spawns workers. Never merge to master without Saksham's explicit word.

## ZedBoard pointer (where focus went)

The ZedBoard (xc7z020) is the hardware-verifiable front now. Bring-up is in
`~/workspace/zedboard-handoff/` (also `~/workspace/zedboard-handoff.zip`),
handoff package on the Mac at `~/Documents/zedboard-helion-debug/`. The v3
tie-off bitstream (`~/workspace/zedboard-hw-loop/bitstreams/zedboard_blinky_v3_tieoff.bit`)
is built and bit-verified but never hardware tested. The loop is: flash with
`openFPGALoader -b zedboard --write-sram <file.bit>`, photograph the board
with the Mac camera, check LD0, iterate. Ownership was with another AI via
the Mac folder; Saksham redirected it to this agent on Nov 8, 2026.
