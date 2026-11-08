# ZedBoard handoff: Helion 7-series backend, from bring-up to a blinking LED

State as of Nov 8, 2026, 12:15 EST. This file supersedes the Oct 22, 2026
`README.md` in the handoff folder, which is stale on the BUFG fix: it
describes the 10-bit BUFG config as a guess and the board as still broken.
The corrected 7-feature BUFG config and the BUFG tie-off fix both landed
after that README was written. Read this file instead.

## The goal

Build a non-Vivado tool that produces bitstreams identical to Vivado's, for
any design, on real AMD/Xilinx parts. The ZedBoard is the main focus right
now because it is real hardware that can verify the work: a bitstream either
blinks the LEDs or it does not. UltraScale+ continues in parallel (see the
UltraScale+ handoff on branch fm/ultra-bitstream); the ZedBoard proves the
7-series backend, UltraScale+ proves the encoding layer.

## Read this first, in this order

1. This file.
2. `~/workspace/zedboard-handoff/TAILSCALE-SETUP.md` (how Muse reaches the
   Mac; the Mac-side steps are the current blocker).
3. `~/workspace/zedboard-crew/BRIEFS/investigator_report.md` (the 22
   anomalies in the backend, the single best technical document on what is
   wrong).
4. `~/workspace/zedboard-crew/SPEC.md`, `INTERFACES.md`, `COORD.md`,
   `CRITIQUE.md` (crew contracts and status).
5. `~/workspace/zedboard-hw-loop/test_manifest.md` (the t01-t05 isolated
   tests and their pass criteria).
6. `~/workspace/zedboard-hw-loop/corpus_manifest.md` (the 21-design corpus,
   shift_chaser first).
7. Then the code: `crates/helion-xilinx7/src/emit.rs` (FASM generation,
   where most bugs live), `src/route.rs`, `src/xdevice.rs`, `src/pack.rs`,
   `src/place.rs`, `src/flow.rs`.

## Current state in one paragraph

Bring-up is done: the xc7z020 device DB, the ZedBoard pin map, and the full
synth to pack to place to route to FASM to bitstream pipeline all run, and
the bitstream programs the board (openFPGALoader reports done=1). A Vivado
bitstream blinks on the same board, so the hardware and JTAG chain are good.
No Helion bitstream has ever blinked an LED. Three Helion attempts went dark
(original, fixed attempt 1, v2). The root cause was found: the BUFGCTRL
control pins were left floating, no clock ever reached the flip-flops. The
fix is built and bit-level verified but never hardware tested. The v3
tie-off bitstream has been waiting since Oct 22, 2026.

## What works

- Device DB: xc7z020, 29,359 tiles, idcode 0x03727093, 9,996 frames
  (branch fm/zedboard-db, commit 4ba283d).
- Pin map verified against the Digilent master XDC: clk Y9 100 MHz; LEDs
  T22 / T21 / U22 / U21 / V22 / W22 / U19 / U14; buttons P16 / R16 / N15 /
  R18 / T18 (branch fm/zedboard-blinky, commit 37b1c14).
- Full pipeline: synth 125 cells (90 LUTs, 27 FFs, 9 IOBs, 1 BUFG) to pack
  to place to route to 826 lines of FASM to a 3,159,295-byte bitstream.
- Programming: `openFPGALoader -b zedboard --write-sram <file.bit>` programs
  the board, done=1. The wrong command is
  `openFPGALoader -c digilent`, which finds 0 devices.
- BUFG polarity fix: the correct I0 to O config is 7 features (IN_USE,
  INIT_OUT, IS_IGNORE0_INVERTED, IS_IGNORE1_INVERTED, ZINV_S0, ZINV_CE0,
  ZPRESELECT_I0). The earlier 10-bit fix was wrong: emitting ZINV_S1,
  ZINV_CE1, PRESELECT_I1 selects the I1 input instead of I0.
- FF CE/SR fix: ZRST per FF when sr_net is present, CEUSEDMUX / SRUSEDMUX
  per slice, deduplicated. 5 emit tests pass.
- BUFG tie-off fix: route.rs calls route_bufg_tieoffs() after normal
  routing; xdevice.rs does DB-driven pip lookup. Verified: unit test
  bufg_tieoff_paths 9 pips pass, e2e zedboard_e2e pass, 10/10 segbits at
  exact frame / word / bit positions, 70 lib tests pass, zero new clippy
  warnings.

## What does not work

- No Helion bitstream has ever blinked an LED on the board.
- The v3 tie-off bitstream was never flashed (blocked on Mac access, see
  below).
- BRAM emits only IN_USE, no port config. BRAM INIT parse failure is
  silently zeroed. DSPs are silently ignored. These are silent
  miscompiles, not loud failures.
- The site allocators (bufg_site, ramb_site, dsp_site) always return the
  first site and ignore the used set, so multi-clock designs collide.
- CARRY4s are placed randomly; they need vertical adjacency.
- LD6 / LD7 may stay dark even on a correct bitstream: RIOB33_SING tiles
  have no segbits in the X-Ray DB. Known gap, excluded from pass criteria.
- No Vivado-vs-Helion bitstream diff has ever been run on this part.

## What we are fixing: the fix list

Every item states what it is, where the code is, and what done looks like.
A fix is done only when its test proves it on hardware or by bit diff.
Nothing here is done on a claim.

1. Flash v3 and get LD0 blinking. What: the BUFG tie-off bitstream
   (~/workspace/zedboard-hw-loop/bitstreams/zedboard_blinky_v3_tieoff.bit,
   3,159,295 bytes). Where: the fix is in route.rs and xdevice.rs on
   branch fm/zedboard-fix-router, commit c62ce79. Done when: LD0 blinks on
   the board. Status: code done and bit-verified, hardware unproven. This
   is the first fix. Everything else waits on it.
2. BUFG I0 config. What: the 7-feature config above. Where: emit.rs, on
   branch fm/zedboard-fix-bufg, commit e0c5ce0 (later verified by 9b1c15f).
   Done when: t02 shows LD0 dim (the clock reaches the FF). Status: code
   done, hardware unproven.
3. FF clock-enable and set/reset. What: ZRST, CEUSEDMUX, SRUSEDMUX. Where:
   emit.rs, branch fm/zedboard-fix-ffctrl, commit b2b4768. Gaps that
   remain: FFSYNC is not implemented; SET vs RESET is hardcoded to RESET.
   Done when: t04 (CE, needs BTNC) and t05 (SR, needs BTNC) pass on the
   board. Status: code done, hardware unproven.
4. CARRY4. What: the CARRY4 emit code is dead code. helion-sv never
   creates Carry4 cells (grep returns 0), so the blinky's 27-bit counter is
   built from LUTs. Full support needs adder inference in helion-sv, DI/S/
   CYINIT capture in pack.rs and place.rs, and PRECYINIT emission in
   emit.rs. Done when: t03 (carry4 counter) counts correctly on the board.
   Status: not started. CARRY4 is not the blinky bug.
5. BRAM port config and loud INIT failure. What: emit real BRAM port
   config instead of only IN_USE, and fail loudly instead of silently
   zeroing a BRAM INIT that fails to parse. Where: emit.rs. Done when: a
   BRAM design works on the board, or the backend refuses to emit it with
   a clear error. Status: not started.
6. DSP fail loud or implement. What: DSPs are silently ignored today
   (`let _ = &placed.dsps;`). Done when: a DSP design either works or the
   backend refuses it loudly. Status: not started.
7. Site allocators respect the used set. What: bufg_site, ramb_site, and
   dsp_site must not return an already-used site. Done when: a two-clock
   design programs and both clocks work. Status: not started.
8. CARRY4 vertical adjacency. What: CARRY4s must be placed in vertically
   adjacent slices. Done when: t03 passes. Status: not started.
9. LD6 / LD7 RIOB33_SING gap. What: those tiles have no X-Ray segbits, so
   LD6 and LD7 cannot be driven by Helion today. Done when: segbits exist
   or the gap is proven unfillable from public data. Status: known gap,
   excluded from every pass criterion.
10. Vivado-vs-Helion bitstream diff on blinky. What: diff the Helion
    bitstream against a Vivado bitstream for the same design, bit by bit.
    Done when: zero diff, or every differing bit is explained. Status:
    never run.
11. IOB output driver. What: verify the IOB config in isolation. Done
    when: t01 passes (LD0-LD4 bright, LD5 dark, no clock involved).
    Status: test written, hardware unproven.

## The hardware loop

This is the loop the user ordered on Nov 8, 2026: flash, open the camera,
check for blink, go back, do it all over again until it is done.

1. The Mac must be awake and unlocked, the ZedBoard powered and plugged
   in, positioned where the Mac's built-in camera can see the LEDs.
2. Muse logs in over Tailscale SSH (see TAILSCALE-SETUP.md).
3. Muse runs `openFPGALoader --detect` to confirm the board shows up.
4. Muse copies the bitstream to the Mac if it is not already there (the Mac
   currently has only the older e0c5ce0 build, `zedboard_blinky_latest.bit`,
   which predates the tie-off fix). Transfer as zip or binary, never as
   text: a text transfer corrupted a bitstream on Oct 22 with UTF-8
   replacement bytes.
5. Muse flashes with `openFPGALoader -b zedboard --write-sram <file.bit>`.
6. Muse takes two camera photos a second apart (camera.snap on the Mac
   device) and inspects them for LD0 blinking. Two photos distinguish a
   blink from a static lit LED.
7. If LD0 blinks, the clock path is fixed. Move to t01-t05, then the
   21-design corpus, shift_chaser first. If LD0 stays dark, the next
   suspects are BUFGCTRL ZINV polarity (hardware validation) or deeper
   clock routing.

## Mac access: the current blocker

As of Nov 8, 2026, Muse cannot log into the Mac. The Tailscale tunnel
answers on port 22 but the Mac never sends an SSH banner, in every client
tested. The most likely cause is Remote Login off on the Mac. The camera
also did not answer (two camera.snap attempts failed; it may need approval
in the Mac app Settings, Permissions, or the camera may be busy). Full
steps, both sides, are in TAILSCALE-SETUP.md. Do not re-run the Nov 8
debugging matrix without new information; the tunnel itself is proven good.

Do NOT re-enable the zedboard-hw-test-loop cron. The user stopped it
deliberately on Oct 22, 2026 ("document everything and stop everything").
The loop now runs on his word, one flash at a time.

## Branch map

- fm/zedboard-db (4ba283d): device DB.
- fm/zedboard-blinky (37b1c14): pin map and blinky RTL.
- fm/zedboard-bringup (d2d067a): full pipeline to bitstream.
- fm/zedboard-fix-bufg (9b1c15f): BUFG polarity fix and FF control
  verification.
- fm/zedboard-fix-ffctrl (d2d067a): FF CE/SR fix.
- fm/zedboard-fix-carry4 (d2d067a): CARRY4 dead-code finding.
- fm/zedboard-fix-router (c62ce79): BUFG tie-off fix. This is the latest
  fix and the current checkout. The v3 bitstream was built from this work.
- origin/zedboard-handoff: the Oct 22 handoff branch on GitHub.

Uncommitted straggler, asked Oct 22, 2026, still unanswered: a 3-line
comment-only change in emit.rs plus untracked
crates/helion-xilinx7/tests/ff_control_bits.rs and
examples/zedboard_static.sv. Do not commit them without the user's word.

## File map

- Repo: https://github.com/helion-fpga/helion. Worktree:
  ~/workspace/helion-zed-bringup. Real git dir: ~/workspace/helion/.git.
- Backend: crates/helion-xilinx7/src/ (emit.rs, route.rs, xdevice.rs,
  pack.rs, place.rs, flow.rs, frames.rs, bitstream.rs, device.rs, fasm.rs,
  timing.rs, lib.rs, backend.rs).
- X-Ray DB: ~/workspace/xilinx-dbsrc (HELION_XILINX_DB), zynq7 family.
  Key DB files: segbits_clk_bufg_bot_r.db, segbits_clbll_l.db.
- Tests: ~/workspace/zedboard-hw-loop/tests/ (t01-t05), test_manifest.md.
- Corpus: ~/workspace/zedboard-hw-loop/corpus_manifest.md (21 designs).
- Bitstreams: ~/workspace/zedboard-hw-loop/bitstreams/
  (zedboard_blinky.bit, zedboard_blinky_fixed.bit, zedboard_blinky_v2.bit,
  zedboard_blinky_v3_tieoff.bit).
- Crew: ~/workspace/zedboard-crew/ (SPEC, COORD, INTERFACES, CRITIQUE,
  BRIEFS including investigator_report.md).
- Handoff folder: ~/workspace/zedboard-handoff/ (README.md is stale, read
  this file instead; the folder also holds the code snapshot, tests, xray-db
  files, findings, and docs). On the Mac:
  ~/Documents/zedboard-helion-debug/ (same package, plus zedboard-fix.bundle
  and zedboard_blinky_latest.bit, the older build).

## History

- Oct 22, 2026: bring-up complete, three dark attempts, root cause found,
  fixes built. The user ordered "document everything and stop everything"
  and the work was handed to another AI with the handoff package.
- Oct 23, 2026: the user said every Helion bitstream was wrong. Asked
  whether to pick the work back up or leave it with the other AI. No
  answer then.
- Nov 8, 2026: the user redirected the ZedBoard to this agent, ordered the
  flash-and-camera loop, and ordered everything saved and pushed. The
  UltraScale+ work was saved to GitHub the same day (commit 3f67f67 on
  fm/ultra-bitstream, all 19 fm/ultra-* branches pushed, nothing merged to
  master).
- Nov 8, 2026: the user handed the coding work to Muse Code and ordered
  this handoff: everything documented, in the respective branches, so a new
  agent can read it and make the fixes without re-analyzing the project.
