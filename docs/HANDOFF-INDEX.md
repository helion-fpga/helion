# Master handoff: the division and where everything is stored

Written Nov 8, 2026, about 13:40 EST. This is the map. It explains how
the handoff is divided into sub-problems, what each one covers, and where
every piece of the work lives on disk and on GitHub. Start here if you are
new. The narrative handoff is docs/HANDOFF.md; this file is the index and
the storage map. If anything disagrees with this file, this file wins on
locations, and the sub-problem file wins on its own sub-problem.

## The goal, in one paragraph

Build a non-Vivado tool that produces bitstreams identical to Vivado's, for
any design, on real AMD/Xilinx parts. Identical means a zero bit-level diff
against a real Vivado bitstream for the same design. The ZedBoard is the
main focus right now because it is real hardware that can verify the work.
UltraScale+ continues in parallel because its encoding layer is the
furthest along.

## How the work is divided

The handoff is divided into 25 sub-problems. Each one is a single file in
subproblems/, written so one agent instance can pick it up and solve it
without re-analyzing the project. Every file has the same shape: goal,
what done looks like, what is already done, what was tried and what
failed, what to do, the exact files and commands, its dependencies, and
the rules. A sub-problem is done only when its "Done when" section is met
and the proof is shown.

### ZedBoard track (13 files, main focus: real hardware)

| ID | File | What it covers | Status |
|----|------|----------------|--------|
| ZB-00 | ZB-00-mac-access.md | Get Muse into the Mac: Tailscale SSH plus camera | Open, blocked, current blocker |
| ZB-01 | ZB-01-flash-v3-blink.md | Flash the v3 tie-off bitstream, get LD0 blinking | Code done, hardware unproven |
| ZB-02 | ZB-02-bufg-i0-config.md | BUFGCTRL I0 config, clock passes to the FFs | Code done, hardware unproven |
| ZB-03 | ZB-03-ff-ce-sr.md | FF clock-enable and set/reset | Code done, hardware unproven |
| ZB-04 | ZB-04-carry4-inference.md | CARRY4 inference in helion-sv | Not started |
| ZB-05 | ZB-05-bram-port-config.md | BRAM port config + loud INIT failure | Not started |
| ZB-06 | ZB-06-dsp-fail-loud.md | DSP fail loud or implement | Not started |
| ZB-07 | ZB-07-site-allocators.md | Site allocators respect the used set | Not started |
| ZB-08 | ZB-08-carry4-adjacency.md | CARRY4 vertical adjacency in placement | Not started |
| ZB-09 | ZB-09-ld6-ld7-sing-gap.md | LD6/LD7 on RIOB33_SING tiles (DB gap) | Known gap, workaround in place |
| ZB-10 | ZB-10-vivado-bitstream-diff.md | Vivado-vs-Helion bitstream diff on blinky | Never run |
| ZB-11 | ZB-11-iob-driver.md | IOB output driver (t01, no clock) | Test written, never flashed |
| ZB-12 | ZB-12-high-medium-cleanup.md | Remaining investigator findings (H/M/L) | Not started |

### UltraScale+ track (12 files, parallel: encoding layer)

| ID | File | What it covers | Status |
|----|------|----------------|--------|
| US-01 | US-01-io-config-tilegrid.md | IO_CONFIG tilegrid + HPIO_RIGHT vs HPIO_L | Blocked (Q6 probe) |
| US-02 | US-02-bram-golden.md | BRAM golden design + INIT translation | Not started, work stalled (see below) |
| US-03 | US-03-dsp-uram-clk-encodings.md | DSP48E2, URAM288, CLK encodings | Blocked (Vivado fuzzing) |
| US-04 | US-04-lutram-table.md | LUTRAM table | Blocked (no public table) |
| US-05 | US-05-carry8-block.md | CARRY8 as its own harness block | Ruled, not implemented |
| US-06 | US-06-xcvu3p-frame-total.md | xcvu3p frame-total gap (73,035 vs 71,820) | Open, provisional |
| US-07 | US-07-random-design-generator.md | Random-design generator + 100-design loop | Not started |
| US-08 | US-08-tier2-vivado-diff.md | Tier 2 Vivado diff (100 designs) | Never run |
| US-09 | US-09-end-to-end-flow.md | End-to-end flow (Verilog in, bitstream out) | Staging only |
| US-10 | US-10-synthesis-qor.md | Synthesis quality (QoR vs Vivado) | Open (23 red, 13 green vs Yosys) |
| US-11 | US-11-bitstream-details.md | Bitstream details (preamble, CRC, padding) | Open |
| US-12 | US-12-harness-speed.md | Harness speed | Open |

### Suggested order

1. ZB-00 first. Nothing hardware can proceed without it.
2. ZB-01 next. One flash answers whether the clock fix was the whole story.
3. ZB-11, ZB-02, ZB-03, then the t04/t05 and t03 tests, in order.
4. In parallel (no hardware needed): ZB-05, ZB-06, ZB-07, ZB-12, US-05,
   US-06, US-12, US-02.
5. US-01 and US-03 wait on the license. US-07 waits on coverage. US-08
   waits on US-07 and the license. US-10 waits on CTO decisions.
6. ZB-04 and ZB-08 go together (inference plus adjacency). ZB-09 and
   ZB-10 go together (the Vivado diff supplies the SING evidence).

### How an instance works a sub-problem

1. Read docs/HANDOFF.md (the narrative), then your sub-problem file,
   then the files it names. Do not re-analyze the project.
2. Work on the branch your file names. One fix per commit.
3. A fix that regresses a green block is reverted on the spot.
4. When the "Done when" section is met, show the proof (command plus
   output, or photos for hardware), get a critic re-run where the file
   asks for one, and update the status line in subproblems/INDEX.md.
5. Never merge to master without the user's explicit instruction.

## Where everything is stored

### The handoff package (this folder)

- Location on this VM: ~/workspace/zedboard-handoff/
- Zip of the whole folder: ~/workspace/zedboard-handoff.zip (refreshed
  every time the folder changes).
- Contents:
  - HANDOFF.md: the fresh combined narrative handoff (goal, both fix
    lists, hardware loop, Mac access, branch map, file map, reading
    list, history). Supersedes all earlier handoffs.
  - HANDOFF-INDEX.md: this file, the master map.
  - TAILSCALE-SETUP.md: the Mac access setup, step by step, both sides,
    with the latest diagnosis. Also committed in the repo as
    docs/tailscale-setup.md.
  - README.md: the Oct 22, 2026 handoff README. STALE on the BUFG fix
    (it describes the 10-bit config as a guess). Read HANDOFF.md instead.
  - subproblems/: all 25 sub-problem files plus INDEX.md (the per-track
    index with the status table and the suggested order).
  - The same package is also on GitHub, on branch
    fm/zedboard-fix-router under docs/zedboard-handoff/ (the code
    snapshot, tests, xray-db, findings, programming instructions, and
    README), so a fresh agent can clone it instead of taking the zip.
    The zip itself is not in git; everything in it is in the repos.
  - code/: snapshot of crates/helion-xilinx7/src (Oct 22, 2026).
  - tests/: t01-t05, zedboard_blinky.sv, zedboard.xdc.
  - xray-db/: segbits_clk_bufg_bot_r.db, segbits_clbll_l.db.
  - findings/: investigator_report.md, critique.md, test_manifest.md,
    corpus_manifest.md.
  - docs/: programming instructions.
- On the Mac: ~/Documents/zedboard-helion-debug/ (same package, plus
  zedboard-fix.bundle, a 34 MB git bundle, and zedboard_blinky_latest.bit,
  the older e0c5ce0 build that predates the tie-off fix).

### The repos (GitHub: https://github.com/helion-fpga/helion)

- Master checkout: ~/workspace/helion-ice40, at 566246a. Real git dir:
  ~/workspace/helion/.git. Worktrees are linked; verify with
  `git worktree list`. Nothing is merged to master without the user's
  explicit instruction.
- ZedBoard worktree: ~/workspace/helion-zed-bringup, branch
  fm/zedboard-fix-router (latest: ce16325). It holds:
  - docs/HANDOFF.md (the combined narrative, same as the folder copy).
  - docs/HANDOFF-INDEX.md (this file).
  - docs/zedboard-handoff.md (the Nov 8 ZedBoard-only handoff,
    superseded by HANDOFF.md).
  - docs/tailscale-setup.md (same as TAILSCALE-SETUP.md).
  - docs/subproblems/ (ZB-00 to ZB-12 plus INDEX.md).
  - crates/helion-xilinx7/src/ (emit.rs, route.rs, xdevice.rs, pack.rs,
    place.rs, flow.rs, frames.rs, bitstream.rs, device.rs, fasm.rs,
    timing.rs, lib.rs, backend.rs).
- UltraScale worktree: ~/workspace/helion-ultra-bitstream, branch
  fm/ultra-bitstream (latest: c02a928). It holds:
  - docs/ultra-bitgen-crew/HANDOFF.md (the UltraScale+ detail handoff).
  - docs/ultra-bitgen-crew/subproblems/ (US-01 to US-12 plus INDEX.md).
  - docs/ultra-bitgen-crew/SPEC.md, INTERFACES.md, COORD.md,
    CRITIQUE.md, BRIEFS/ (including tilegrid-research.md and
    bram-golden-build.md).
  - crates/helion-ultrascale/src/ (bitgen.rs, segbits.rs, bitloc.rs,
    device_file.rs, bitgen_device.rs, bin/helion-bitgen.rs).
  - crates/helion-ultrascale/harness/ (the diff harness snapshot:
    diff.py, diff_engine.py, emitter.py, fasm.py, gen_golden.py,
    reference.py, roundtrip.py, tests/, golden/).
  - crates/helion-ultrascale/data/ (vendored reference data; the loader
    never reads /tmp).
- All 7 fm/zedboard-* branches are on GitHub: db (4ba283d), blinky
  (37b1c14), bringup (d2d067a), fix-bufg (9b1c15f), fix-ffctrl (d2d067a),
  fix-carry4 (d2d067a), fix-router (ce16325). Plus origin/zedboard-handoff
  (the Oct 22 branch).
- All 19 fm/ultra-* branches are on GitHub, plus the new
  fm/ultra-bitgen-bramgolden (orchestrator 15, worktree
  ~/workspace/helion-ultra-bitgen-bramgolden, at base e38158f, with
  uncommitted BRAM work and a compile error E0027 in
  tests/bram_emit.rs:226; see US-02 and the run log below).

### The hardware loop (ZedBoard)

- ~/workspace/zedboard-hw-loop/
  - tests/ (t01_iob_static.sv, t02_bufg_toggle.sv, t03_carry4_counter.sv,
    t04_ff_ce.sv, t05_ff_sr.sv), test_manifest.md (pass criteria),
    corpus_manifest.md (21 designs, shift_chaser first).
  - bitstreams/ (zedboard_blinky.bit, zedboard_blinky_fixed.bit,
    zedboard_blinky_v2.bit, zedboard_blinky_v3_tieoff.bit, 3,159,295
    bytes, the one that matters).
  - ssh_proxy2.sh (the durable tunnel script; the /tmp copy is wiped on
    every VM restart, restore it from here).
  - test_loop.sh, logs/, emit_audit_report.md.
- The zedboard-hw-test-loop cron is DISABLED on purpose (the user
  stopped it Oct 22, 2026). Do NOT re-enable it. The loop runs on the
  user's word, one flash at a time.

### The crews and their logs

- ~/workspace/zedboard-crew/ (SPEC, COORD, INTERFACES, CRITIQUE, BRIEFS/
  including investigator_report.md, the single best technical document
  on what is wrong with the 7-series backend).
- ~/workspace/ultra-bitgen-crew/ (SPEC, INTERFACES, COORD, CRITIQUE,
  BRIEFS/, harness/). The live harness (not the snapshot) is at
  ~/workspace/ultra-bitgen-crew/harness/, with runs/ (317 MB of 16.5 MB
  bitstreams, deliberately not pushed) and reference/ (33 MB, deliberately
  not pushed; vendored equivalents are in crates/helion-ultrascale/data/).
- Day-1 crews (Oct 22, 2026, read-only history): ~/workspace/ultrascale-
  crew/, ~/workspace/qor2-crew/, ~/workspace/vivado-study/,
  ~/workspace/release-3.5/.
- Watcher log: ~/workspace/watcher/WATCHER.md (crew-watcher cron, every
  5 minutes, reads the 4 day-1 COORD files only; its "license refused"
  line is stale, the license state is unknown, see below).

### The drivers and their logs

- bitgen-100-driver (cron, hourly, goal
  virtex-ultrascale-backend-support): definition at
  ~/workspace/goals/virtex-ultrascale-backend-support/crons/hourly/
  bitgen-100-driver__interval@1h.md; run log at
  ~/workspace/goals/virtex-ultrascale-backend-support/hidden_files/
  bitgen-100-driver/run-log.md. It works the UltraScale+ queue until
  100 random designs are Vivado-equivalent at both tiers. It reports
  only on milestones (a block green, a critic FAIL, a new blocker, every
  10-design tier-1 milestone, tier-1 100/100, tier-2 progress).
- crew-watcher (cron, every 5 minutes, goal crew-progress-watcher):
  definition at ~/workspace/goals/crew-progress-watcher/crons/minutely/
  crew-watcher__interval@5m.md.

### Reference data (do not vendor without provenance)

- ~/workspace/xilinx-dbsrc (HELION_XILINX_DB, zynq7 family; key files
  segbits_clk_bufg_bot_r.db, segbits_clbll_l.db).
- ~/workspace/prjuray-db-ref (f4pga/prjuray, CC0 1.0, read-only; no
  UltraScale+ reference.bit files, 7-series only).
- ~/workspace/prjuray-tools-ref (f4pga/prjuray, read-only, Apache-2.0).
- bitfiltrator-ref/ (untracked in the helion-ultra-bitstream worktree;
  MIT, EPFL VLSC 2022; ULTRASCALE_PLUS.json SHA-256
  c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c).
- PROVENANCE.md in the ultrascale crew docs is the single provenance
  index.

### Vivado and the license

- Vivado 2017.3 on the USF cluster, host sc.rc.usf.edu, login
  srivastava1. The sandbox cannot reach it (campus / VPN only).
- License server 27000@license2.forest.usf.edu. State: UNKNOWN. The Nov
  8 "port answered" report was proven VOID about 13:30 EST by a control
  test (direct TCP connects from this VM succeed to every port, including
  known-closed ones, so the check proves nothing). Only a real Vivado
  checkout on the cluster can verify it. Never claim Vivado-equivalence
  without a real Vivado bitstream diff at zero.
- Fuzzing package: ~/workspace/your_files/vivado-fuzzing-instructions.md
  plus vivado-fuzzing-package.zip (30 files, jobs 1-5; job 1 is the
  xcvu3p device file, top priority).

### Memory and user files

- ~/MEMORY.md (curated long-term memory), ~/memory/2026-11-08.md (daily
  log), ~/memory/people/ and ~/memory/groups/ (relationship index).
- ~/workspace/your_files/ (the canonical resume, the fuzzing package,
  and other user files).

## Current state, brief

- Goal: Vivado-identical bitstreams without Vivado. ZedBoard is the main
  focus; UltraScale+ runs in parallel.
- ZedBoard: v3 tie-off bitstream built and bit-verified, never flashed.
  Blocked on ZB-00 (Mac access: Remote Login is ON and the key is
  installed, but SSH still fails on every path; the next suspects are
  the macOS firewall and Tailscale SSH; the deciding test is
  `ssh saksham@localhost` on the Mac).
- UltraScale+: 153 tests pass on fm/ultra-bitstream. Tier 1 green on
  xcku3p for blinky and counter: LUT_INIT, FF_INIT, INT_PIP, FRAMING.
  Blocked: IO_CONFIG (tilegrid), BRAM/DSP/URAM/CLK (unexercised),
  LUTRAM. Orchestrator session 15 (BRAM golden) stopped on a cron
  timeout with uncommitted changes and a compile error E0027; resume
  order is one line, re-run tests, commit golden files, harness, critic.
- License: UNKNOWN (see above). Tier 2 has never run.

## Standing rules

- No em dashes anywhere, not in chat, files, READMEs, artifacts, or code
  comments.
- Never fabricate terminal output, visuals, or numbers. Every claim needs
  its command plus output shown.
- Never merge to master without the user's explicit instruction.
- Every fix ships with a test that proves it. A fix that regresses a
  green block is reverted on the spot.
