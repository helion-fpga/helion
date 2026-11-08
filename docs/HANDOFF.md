# Handoff: Helion, Vivado-identical bitstreams without Vivado

Fresh handoff, written Nov 8, 2026, about 12:55 EST. This file supersedes
all earlier handoffs: the Oct 22, 2026 ZedBoard README, the Nov 8 ZedBoard
handoff (docs/zedboard-handoff.md), the Nov 8 UltraScale+ handoff
(docs/ultra-bitgen-crew/HANDOFF.md on branch fm/ultra-bitstream), and the
Nov 8 Tailscale setup doc. Those files stay on disk for history, but if
anything disagrees with this file, this file wins. A new agent should be
able to start from here without re-analyzing the project.

The handoff is divided into 25 sub-problems, each a standalone file, and
the storage map is docs/HANDOFF-INDEX.md: read it to find where every
file, branch, worktree, crew log, bitstream, and data set lives.

## The goal, in plain words

Build a non-Vivado tool that produces bitstreams identical to Vivado's, for
any design, on real AMD/Xilinx parts. Identical means a zero bit-level diff
against a real Vivado bitstream for the same design, not just against public
databases. The ZedBoard is the main focus right now because it is real
hardware that can verify the work: a bitstream either blinks the LEDs or it
does not. UltraScale+ continues in parallel because its encoding layer is
the furthest along. The goal is not "better than Yosys". The goal is Vivado,
exactly.

## What we are trying to do

Two tracks. The ZedBoard track proves the 7-series backend on real silicon.
The UltraScale+ track proves the bitstream encoding layer against public
databases and, when the license allows it, against real Vivado bitstreams.

### Track 1: ZedBoard (main focus)

The board is a Digilent ZedBoard, FPGA xc7z020clg484-1, 100 MHz clock on pin
Y9, 8 LEDs on pins T22 / T21 / U22 / U21 / V22 / W22 / U19 / U14, 5 buttons
on P16 / R16 / N15 / R18 / T18. Programmer is openFPGALoader 1.2.0.

Current state: bring-up is done (device DB, pin map, full pipeline runs,
bitstream programs the board, done=1). A Vivado bitstream blinks on the same
board, so the hardware and JTAG chain are good. No Helion bitstream has ever
blinked an LED. Three Helion attempts went dark. The root cause was found:
the BUFGCTRL control pins were left floating, so no clock ever reached the
flip-flops. The fix is built and bit-level verified but never hardware
tested. The v3 tie-off bitstream has been waiting since Oct 22, 2026.

The fixes, in order. Each states what it is, where the code is, and what
done looks like. A fix is done only when its test proves it on hardware or
by bit diff. Nothing is done on a claim.

1. Flash v3 and get LD0 blinking. The bitstream is
   ~/workspace/zedboard-hw-loop/bitstreams/zedboard_blinky_v3_tieoff.bit
   (3,159,295 bytes). The fix is in crates/helion-xilinx7/src/route.rs and
   src/xdevice.rs on branch fm/zedboard-fix-router, commit c62ce79. Done
   when LD0 blinks on the board. Status: code done and bit-verified,
   hardware unproven. This is the first fix. Everything else waits on it.
2. BUFG I0 config. The correct I0 to O config is 7 features (IN_USE,
   INIT_OUT, IS_IGNORE0_INVERTED, IS_IGNORE1_INVERTED, ZINV_S0, ZINV_CE0,
   ZPRESELECT_I0). The earlier 10-bit fix was wrong: emitting ZINV_S1,
   ZINV_CE1, PRESELECT_I1 selects the I1 input instead of I0. Code is in
   emit.rs on branch fm/zedboard-fix-bufg. Done when test t02 shows LD0 dim
   (the clock reaches the FF). Status: code done, hardware unproven.
3. FF clock-enable and set/reset. ZRST per FF when sr_net is present,
   CEUSEDMUX / SRUSEDMUX per slice, deduplicated. Code is in emit.rs on
   branch fm/zedboard-fix-ffctrl, commit b2b4768. Gaps that remain: FFSYNC
   is not implemented; SET vs RESET is hardcoded to RESET. Done when tests
   t04 (CE, needs BTNC) and t05 (SR, needs BTNC) pass on the board. Status:
   code done, hardware unproven.
4. CARRY4. The CARRY4 emit code is dead code. helion-sv never creates
   Carry4 cells (grep returns 0), so the blinky's 27-bit counter is built
   from LUTs. Full support needs adder inference in helion-sv, DI/S/CYINIT
   capture in pack.rs and place.rs, and PRECYINIT emission in emit.rs.
   Done when test t03 (carry4 counter) counts correctly on the board.
   Status: not started. CARRY4 is not the blinky bug.
5. BRAM port config and loud INIT failure. BRAM emits only IN_USE, no port
   config. A BRAM INIT parse failure is silently zeroed. Done when a BRAM
   design works on the board, or the backend refuses to emit it with a
   clear error. Status: not started.
6. DSP fail loud or implement. DSPs are silently ignored today
   (`let _ = &placed.dsps;`). Done when a DSP design either works or the
   backend refuses it loudly. Status: not started.
7. Site allocators respect the used set. bufg_site, ramb_site, and dsp_site
   always return the first site and ignore the used set, so multi-clock
   designs collide. Done when a two-clock design programs and both clocks
   work. Status: not started.
8. CARRY4 vertical adjacency. CARRY4s must be placed in vertically
   adjacent slices. Done when t03 passes. Status: not started.
9. LD6 / LD7 RIOB33_SING gap. Those tiles have no X-Ray segbits, so LD6 and
   LD7 cannot be driven by Helion today. Done when segbits exist or the gap
   is proven unfillable from public data. Status: known gap, excluded from
   every pass criterion.
10. Vivado-vs-Helion bitstream diff on blinky. Diff the Helion bitstream
    against a Vivado bitstream for the same design, bit by bit. Done when
    zero diff, or every differing bit is explained. Status: never run.
11. IOB output driver. Done when test t01 passes (LD0-LD4 bright, LD5
    dark, no clock involved). Status: test written, hardware unproven.

The isolated tests live in ~/workspace/zedboard-hw-loop/tests/ (t01_iob_static,
t02_bufg_toggle, t03_carry4_counter, t04_ff_ce, t05_ff_sr), with pass criteria
in ~/workspace/zedboard-hw-loop/test_manifest.md. Run them in order; each
isolates one primitive so a failure points at exactly one fix. After t01-t05
pass, run the 21-design corpus in
~/workspace/zedboard-hw-loop/corpus_manifest.md, shift_chaser first.

### Track 2: UltraScale+ bitgen (parallel)

Target parts: xcku3p first (full public device file), then xcvu3p
(provisional). The pipeline takes hand-written FASM in and emits a bitstream.
A diff harness compares the emitted bits against public segbit databases
(tier 1) and, when possible, against real Vivado bitstreams (tier 2).

Current state: 153 tests pass on branch fm/ultra-bitstream. Tier 1 green
on xcku3p for blinky and counter: LUT_INIT, FF_INIT, INT_PIP, FRAMING.
Emitted bitstreams are full-device, 16,585,732 bytes per design. A full
harness run takes about 9 minutes. An hourly driver (cron job
bitgen-100-driver) keeps working the queue until 100 random designs are
Vivado-equivalent at both tiers.

The fixes, in order. A fix is done only when its test proves it.

1. IO_CONFIG tilegrid. The xcku3p tilegrid, the map from tile names to
   absolute frame addresses. Largest blocked block (55 bits). Done when
   IO_CONFIG reads green on both designs. Status: blocked. No tilegrid
   exists in any public source, and the device file says HPIO_L, not
   HPIO_RIGHT. Unblocker: the Q6 Vivado probe (see the license section).
2. BRAM golden design. A design that uses block RAM, plus the whole-array
   INIT to per-word INIT translation the DB expects. Done when BRAM_CONTENT
   and BRAM_PARITY read green. Status: not started.
3. DSP48E2, URAM288, CLK encodings. Done when each block reads green.
   Status: blocked, needs Vivado fuzzing (license).
4. LUTRAM table. Done when LUTRAM reads green, or it is proven that only
   Vivado logic-location data can supply the table. Status: blocked, no
   public table exists.
5. CARRY8 harness block. CARRY8 as its own block in the harness registry
   and the FASM classifier, never folded into LUT_INIT. Done when the block
   exists and reports green/red on designs that use CARRY8. Status: ruled,
   not implemented.
6. xcvu3p frame-total gap. Our computed frame total is 73,035 against
   UG570 Table 1's 71,820. Done when resolved by Vivado confirmation.
   Status: open, flagged provisional, never tuned away.
7. Random-design generator and the 100-design loop. A seeded generator
   producing random valid FASM designs covering every block, plus the
   harness loop that diffs them. Done when 100/100 green at tier 1.
   Status: not started.
8. Tier 2 Vivado diff. A zero diff against real Vivado bitstreams for the
   same 100 designs. Done when 100/100 green at tier 2. Status: never run,
   license checkout unverified.
9. End-to-end flow. Verilog in, bitstream out: ROUTE, a real WireDb, the
   router-to-PIP translator, real pack/place, and a real timing DB. Done
   when a Verilog design flows to a bitstream through Helion on this part.
   Status: ROUTE fails loud, WireDb empty, translator not written,
   pack/place are staging code.
10. Synthesis quality. A netlist as good as Vivado's: the K=4 mapper
    repair, Carry4 adder inference, BRAM inference, the picorv32 frontend
    loop, the cv32e40p gaps. Done when the QoR scoreboard (currently 23 red,
    13 green vs Yosys on 36 designs) is green against Vivado. Status: open,
    owned by the QoR campaign, not this crew.
11. Bitstream details. The preamble registers Vivado writes, the exact
    ending sequence, the frame-0 CRC state, the last-of-row 2-zero-padding
    path, the all-zero-value parse limit. Done when each is observed from a
    real Vivado bitstream or proven correct. Status: preamble not emitted
    on purpose (values unknown), ending mirrored from 7-series, frame-0 CRC
    derived by linear algebra, padding never exercised.
12. Harness speed. A full run takes about 9 minutes for 2 designs, mostly
    walking 16.5 MB bitstreams. Done when a 2-design run is a few minutes
    at most. Status: open, performance only, outputs are correct.

Key facts for this track: the FASM format is `<tile>.<feature> [= <value>]`;
FF INIT uses the canonical .V0/.V1 spelling, with bit = NOT(INIT) and bit =
NOT(SRVAL); the SRVAL bit derives from the INIT bit by a within-slice delta
(-1 for A/C/E/G letters, +1 for B/D/F/H letters); the HPIO bridge maps
HPIO_RIGHT_X0Y0 to base FAR 0x000, an invented value that dies when the
tilegrid lands; the preamble registers Vivado writes are deliberately NOT
emitted because UltraScale+ values are unknown in any open source; guessed
values are never emitted.

## How to get it to work: the hardware loop

This is the loop the user ordered on Nov 8, 2026: flash, open the camera,
check for blink, go back, do it all over again until it is done.

1. The Mac must be awake and unlocked, the ZedBoard powered and plugged
   in, positioned where the Mac's built-in camera can see the LEDs.
2. Muse logs in over Tailscale SSH (see the next section).
3. Muse runs `openFPGALoader --detect` to confirm the board shows up.
4. Muse copies the bitstream to the Mac if it is not already there. The Mac
   currently has only the older e0c5ce0 build, `zedboard_blinky_latest.bit`,
   which predates the tie-off fix. Transfer as zip or binary, never as
   text: a text transfer corrupted a bitstream on Oct 22 with UTF-8
   replacement bytes.
5. Muse flashes with `openFPGALoader -b zedboard --write-sram <file.bit>`.
   The wrong command is `openFPGALoader -c digilent`, which finds 0
   devices. openFPGALoader 1.2.0 is installed on the Mac via Homebrew, at
   /opt/homebrew/bin, which is not on the default SSH PATH, so use the full
   path if the plain command is not found.
6. Muse takes two camera photos a second apart (camera.snap on the Mac
   device) and inspects them for LD0 blinking. Two photos distinguish a
   blink from a static lit LED.
7. If LD0 blinks, the clock path is fixed. Move to t01-t05, then the
   21-design corpus, shift_chaser first. If LD0 stays dark, the next
   suspects are BUFGCTRL ZINV polarity (hardware validation) or deeper
   clock routing.

Do NOT re-enable the zedboard-hw-test-loop cron. The user stopped it
deliberately on Oct 22, 2026 ("document everything and stop everything").
The loop now runs on his word, one flash at a time.

## How to get it to work: Mac access over Tailscale

This is the current blocker, and the part that changed most recently. Read
the whole section; the diagnosis changed twice on Nov 8.

### The setup (done)

Muse's side is done:
- This VM is enrolled on the user's tailnet as `muse`, Connected.
- SSH private key saved at ~/.ssh/tailscale_mac. The matching public key is
  `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAiosL4sBt81zC3KHfCpS44271bmDnh8l4kg/Imtp5Z+ muse-vm-to-mac`.
- Tunnel script saved at ~/workspace/zedboard-hw-loop/ssh_proxy2.sh. The
  copy in /tmp is wiped on every VM restart, so it is restored from the
  durable copy before use.
- The exact login command:
  `ssh -i ~/.ssh/tailscale_mac -o StrictHostKeyChecking=no -o BatchMode=yes -o ConnectTimeout=30 -o ProxyCommand="/tmp/ssh_proxy2.sh %h %p" saksham@100.70.59.57`
- The Mac shows on the tailnet as `sakshams-macbook-pro` at 100.70.59.57,
  IPv6 fd7a:115c:a1e0::8a35:3b3a. Its name also resolves to a USF campus
  address, 198.18.111.128.

The user's side, done Nov 8, 2026, about 12:30 EST:
- Tailscale installed and signed in on the Mac.
- Remote Login ON (System Settings, General, Sharing). Verified with
  `sudo systemsetup -getremotelogin`, which reports On.
- Muse's public key installed in ~/.ssh/authorized_keys on the Mac (192
  bytes, modified Nov 8 12:30 EST). Installed with:
  `mkdir -p ~/.ssh && chmod 700 ~/.ssh`, then the echo of the key line into
  ~/.ssh/authorized_keys, then `chmod 600 ~/.ssh/authorized_keys`.

### The problem (still open as of Nov 8, 12:45 EST)

SSH still fails on every path tried, even with Remote Login on and the key
installed:
- IPv4 via the Tailscale tunnel: the connection opens, nothing ever talks
  back, empty close after about 8 seconds.
- IPv6 via the Tailscale tunnel: TCP connects, but the banner never
  arrives ("Connection timed out during banner exchange").
- USF campus address 198.18.111.128 via the egress proxy: empty close.
- Earlier tests (curl, Python, socat, nc) all showed the same thing: the
  tunnel answers on port 22, but the Mac never sends an SSH banner. The
  requests were proven byte-identical between clients, so it is not a
  client problem.

Remote Login being off is now ruled out. The next suspects are:
- The macOS firewall blocking incoming SSH. Check on the Mac: System
  Settings, Network, Firewall, or
  `sudo /usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate`
  and `--getblockall`. If "Block all incoming connections" is on, SSH is
  blocked even with Remote Login on.
- Tailscale SSH intercepting port 22. If Tailscale SSH is enabled on the
  Mac or in the tailnet admin console, the Tailscale daemon grabs port 22
  and only allows nodes the tailnet ACL names. Check the Tailscale app on
  the Mac for a Tailscale SSH setting, and the admin console ACL for an
  ssh rule covering the `muse` node to `sakshams-macbook-pro` as user
  `saksham`.

### The test that settles it

Run on the Mac itself, in Terminal:
`ssh saksham@localhost`
If that logs in, sshd is fine and the problem is the firewall or Tailscale.
If it fails, sshd itself is the problem. Also run:
`sudo lsof -iTCP:22 -sTCP:LISTEN`
to show whether anything is listening on port 22 at all.

### Camera

Two camera.snap attempts failed on Nov 8 (first timed out waiting on
camera access, second hit a 120 s timeout). The camera may need approval in
the Mac app Settings, Permissions, or it may be busy with another app. The
camera cannot work while another app uses it.

### How to cut access

Any one of these stops Muse from reaching the Mac: turn off Remote Login,
remove the `muse-vm-to-mac` line from ~/.ssh/authorized_keys, sign out of
Tailscale on the Mac, or lock the Mac / let it sleep.

## License and Vivado access

Vivado 2017.3 lives on the USF cluster, host sc.rc.usf.edu, login
srivastava1. The sandbox cannot reach it (campus / VPN only). The license
server is 27000@license2.forest.usf.edu. It refused connections from Oct 22
through Nov 7, 2026. On Nov 8 at about 10:30 EST the port appeared to
answer, but that finding was proven VOID later the same day (about 13:30
EST): a control test showed direct TCP connects from this VM succeed to
every port, including known-closed ones (license2:1, example.com:81), so
the sandbox gateway accepts the handshake and the check proves nothing
about the license server. The license state is UNKNOWN. A real license
checkout is still unverified: no lmutil or Vivado exists on this VM. Never
claim Vivado-equivalence without a real Vivado bitstream diff at zero.

The Q6 Vivado probe is the unblocker for the UltraScale+ tilegrid. It is a
Tcl probe that runs on the cluster: step 1, tile inventory (extract_device_
info.tcl plus `TILE_TYPE =~ HPIO*` queries, which alone settles HPIO_RIGHT
vs HPIO_L); step 2, one row-0 HPIO tile base FAR plus frame count; step 3,
the offset ruling. It cannot run from this VM. The fuzzing package (jobs 1-5,
top priority the xcvu3p device file) is at
~/workspace/your_files/vivado-fuzzing-instructions.md plus
vivado-fuzzing-package.zip (30 files).

## Branch map

ZedBoard branches (all pushed to GitHub, https://github.com/helion-fpga/helion):
- fm/zedboard-db (4ba283d): device DB.
- fm/zedboard-blinky (37b1c14): pin map and blinky RTL.
- fm/zedboard-bringup (d2d067a): full pipeline to bitstream.
- fm/zedboard-fix-bufg (9b1c15f): BUFG polarity fix and FF control
  verification.
- fm/zedboard-fix-ffctrl (d2d067a): FF CE/SR fix.
- fm/zedboard-fix-carry4 (d2d067a): CARRY4 dead-code finding.
- fm/zedboard-fix-router (5805855): BUFG tie-off fix plus the Nov 8 handoff
  docs. This is the latest fix and the current checkout. The v3 bitstream
  was built from this work.
- origin/zedboard-handoff: the Oct 22 handoff branch.

UltraScale+ branches (all 19 fm/ultra-* branches pushed, nothing merged to
master):
- fm/ultra-bitstream (e38158f): base, holds the handoff, crew docs, and the
  harness snapshot. Code tip 060b230.
- Worker branches: fm/ultra-bitgen-core (5e26453), fm/ultra-bitgen-device
  (33a4b8f), fm/ultra-bitgen-segbits (c1bef40), fm/ultra-bitgen-lut
  (0e81099), fm/ultra-bitgen-lutff (7f80b54), fm/ultra-bitgen-int (6bb30b6),
  fm/ultra-bitgen-diff (aee77e3), fm/ultra-bitgen-integ (f5ebcf7),
  fm/ultra-bitgen-bitgendev (4fd7120), fm/ultra-bitgen-emitter (9290ce6),
  fm/ultra-bitgen-tilegrid (a63f49c), plus fm/ultra-device (7a4146f),
  fm/ultra-multiseed, fm/ultra-packplace, fm/ultra-qor, fm/ultra-router,
  fm/ultra-routing, fm/ultra-timing.

Uncommitted straggler on the ZedBoard side, asked Oct 22, 2026, still
unanswered: a 3-line comment-only change in emit.rs plus untracked
crates/helion-xilinx7/tests/ff_control_bits.rs and
examples/zedboard_static.sv. Do not commit them without the user's word.

## File map

- Repo: https://github.com/helion-fpga/helion. Master checkout
  ~/workspace/helion-ice40 at 566246a. Real git dir: ~/workspace/helion/.git.
  Worktrees are linked; verify with `git worktree list`.
- ZedBoard worktree: ~/workspace/helion-zed-bringup. Backend:
  crates/helion-xilinx7/src/ (emit.rs, route.rs, xdevice.rs, pack.rs,
  place.rs, flow.rs, frames.rs, bitstream.rs, device.rs, fasm.rs, timing.rs,
  lib.rs, backend.rs). X-Ray DB: ~/workspace/xilinx-dbsrc
  (HELION_XILINX_DB), zynq7 family; key files segbits_clk_bufg_bot_r.db and
  segbits_clbll_l.db.
- UltraScale worktree: ~/workspace/helion-ultra-bitstream. Crate:
  crates/helion-ultrascale/src/ (bitgen.rs, segbits.rs, bitloc.rs,
  device_file.rs, bitgen_device.rs, bin/helion-bitgen.rs). Harness:
  crates/helion-ultrascale/harness/ (diff.py, diff_engine.py, emitter.py,
  fasm.py, gen_golden.py, reference.py, roundtrip.py, tests/, golden/).
  Crew docs: docs/ultra-bitgen-crew/ (SPEC, INTERFACES, COORD, CRITIQUE,
  BRIEFS including tilegrid-research.md). Crew dir:
  ~/workspace/ultra-bitgen-crew/.
- ZedBoard crew: ~/workspace/zedboard-crew/ (SPEC, COORD, INTERFACES,
  CRITIQUE, BRIEFS including investigator_report.md, the single best
  technical document on what is wrong with the backend).
- Tests and bitstreams: ~/workspace/zedboard-hw-loop/ (tests/t01-t05,
  test_manifest.md, corpus_manifest.md, bitstreams/, ssh_proxy2.sh,
  test_loop.sh).
- Handoff folder: ~/workspace/zedboard-handoff/ (this file, the code
  snapshot, tests, xray-db files, findings, docs). On the Mac:
  ~/Documents/zedboard-helion-debug/ (same package, plus
  zedboard-fix.bundle and zedboard_blinky_latest.bit, the older build).
- QoR campaign: ~/workspace/qor2-crew/ (SPEC, COORD, harness in
  ~/workspace/qor-campaign/coord/harness/). Honest scoreboard: 23 red, 13
  green vs Yosys on 36 designs. Definition of done, locked Oct 22, 2026:
  beat Yosys on ALL designs, ALL families, ALL levels, per design.

## Read this first, in this order

1. This file.
2. ~/workspace/zedboard-crew/BRIEFS/investigator_report.md (the 22
   anomalies in the 7-series backend).
3. docs/ultra-bitgen-crew/SPEC.md and INTERFACES.md (the UltraScale+
   definition of done and contracts).
4. The tail of docs/ultra-bitgen-crew/COORD.md and the latest session in
   CRITIQUE.md (the newest queue state and critic verdict).
5. docs/ultra-bitgen-crew/BRIEFS/tilegrid-research.md (the tilegrid
   question and the Q6 probe).
6. Then the code, starting with crates/helion-xilinx7/src/emit.rs and
   crates/helion-ultrascale/src/bitgen.rs.

## Standing rules

- No em dashes anywhere, not in chat, files, READMEs, artifacts, or code
  comments. Use hyphens, commas, colons, or restructure.
- Never fabricate terminal output, visuals, or numbers. Every engineering
  claim needs the command plus output shown. Nothing is done or tested
  without a shown run. No "likely" or "probably" in status reports.
- Never merge to master without the user's explicit instruction.
- Every fix ships with a test that proves it. A fix that regresses a green
  block is reverted on the spot.
- Passwords and logins go in Secure Vault only, never chat. The SSH key in
  this file is the public key only; the private key never leaves Muse's
  side.

## History, brief

- Oct 22, 2026: ZedBoard bring-up complete, three dark attempts, root cause
  found, fixes built. The user ordered "document everything and stop
  everything" and the work was handed to another AI with the handoff
  package. UltraScale+ day-1 crew and QoR sprint 3 finished the same day.
- Oct 23, 2026: the user said every Helion bitstream was wrong. Asked
  whether to pick the work back up or leave it with the other AI. No answer
  then.
- Nov 7, 2026: UltraScale+ bitgen crew launched. LUT/FF/INT green tier 1.
- Nov 8, 2026: FRAMING green formal (SRVAL fix, critic 15 PASS); INT
  column rule corrected (critic 16 PASS); the user ordered fix-everything plus a
  100-design driver (cron bitgen-100-driver, hourly); focus shifted to the
  ZedBoard; everything saved and pushed to GitHub; the user handed the
  coding work to Muse Code and ordered this handoff; the Tailscale setup
  went through two diagnosis rounds (Remote Login off, then Remote Login on
  but still failing, firewall or Tailscale SSH now suspected).
- Nov 8, 2026, about 13:30 EST: correction, the license-port finding is
  void (see the license section). The bitgen-100 driver run at 12:27 EST
  spawned orchestrator session 15 for the BRAM golden design; it stopped
  on a cron timeout with uncommitted changes (bitgen.rs BRAM emit branch,
  tests/bram_emit.rs) and a compile error E0027 in tests/bram_emit.rs:226,
  zero commits, nothing green. Resume order: fix the E0027 pattern (one
  line), re-run the bram_emit tests, commit the golden files, then run
  the harness and request the critic.
