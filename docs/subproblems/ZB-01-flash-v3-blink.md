# ZB-01: Flash the v3 tie-off bitstream and get LD0 blinking

- Track: ZedBoard, hardware. The first fix. Everything else waits on it.
- Branch: fm/zedboard-fix-router, tip 09450b8 (handoff docs) / code tip
  c62ce79. Worktree: ~/workspace/helion-zed-bringup.
- Status: code done and bit-verified, hardware unproven. Blocked on ZB-00
  (Mac access).

## Goal

Prove the BUFG tie-off fix on real hardware: flash the v3 bitstream onto
the ZedBoard and see LD0 blink.

## Done when

LD0 blinks on the board after flashing
~/workspace/zedboard-hw-loop/bitstreams/zedboard_blinky_v3_tieoff.bit
(3,159,295 bytes), confirmed by two camera photos a second apart (one with
LD0 lit, one with it dark, or a visible change between the two). A single
photo of a lit LED does not prove a blink.

## What is already done

- Bring-up complete Oct 22, 2026: xc7z020 device DB (29,359 tiles, idcode
  0x03727093, 9,996 frames), pin map verified against the Digilent master
  XDC (clk Y9 100 MHz; LEDs T22 / T21 / U22 / U21 / V22 / W22 / U19 / U14;
  buttons P16 / R16 / N15 / R18 / T18), full pipeline synth to bitstream
  (125 cells: 90 LUTs, 27 FFs, 9 IOBs, 1 BUFG; 826 lines of FASM;
  3,159,295-byte bitstream).
- Root cause found Oct 22, 2026, about 10:17 PM EDT: the BUFGCTRL control
  pins (S0 / S1 / CE0 / CE1 / IGNORE0 / IGNORE1) were never tied off.
  Config bits were set but the physical pins were left floating, so no
  clock ever reached the flip-flops.
- Fix built on branch fm/zedboard-fix-router, commit c62ce79: route.rs
  calls route_bufg_tieoffs() after normal routing; xdevice.rs does
  DB-driven pip lookup (constants from neighboring INT_R_X33Y46 TIEOFF
  site GND_WIRE / VCC_WIRE rails; the CLK_BUFG tile has no TIEOFF wires).
- Fix verified at bit level: unit test bufg_tieoff_paths 9 pips pass, e2e
  zedboard_e2e pass (864 s), 10/10 segbits at exact frame / word / bit
  positions, 70 lib tests pass, zero new clippy warnings.
- Programming is proven: `openFPGALoader -b zedboard --write-sram <file.bit>`
  programs the board, done=1. The wrong command is
  `openFPGALoader -c digilent`, which finds 0 devices.
- A Vivado bitstream blinks on the same board, so the hardware and JTAG
  chain are good.

## What was tried and what failed

Three Helion attempts went dark on Oct 22, 2026:
- Original blinky: dark (logged about 20:41 EDT).
- Fixed attempt 1: dark (about 20:55 EDT).
- v2 (10-bit BUFG plus CARRY4 mux): dark (confirmed 21:15 EDT).
- Original blinky round-robin: dark again (confirmed 21:24 EDT).
All programmed fine (done=1). The board proved the bitstreams wrong three
times. The user ordered "document everything and stop everything" at about
9:25 PM EDT Oct 22, and the zedboard-hw-test-loop cron was disabled. Do NOT
re-enable it; the loop now runs on the user's word, one flash at a time.

A corrupted download incident Oct 22, about 8:05 PM EDT: a bitstream
transferred as text came back with EF BF BD (UTF-8 replacement) sequences.
The original file was clean (SHA256
d0815609f6b8fd8c39a95edac124c1e57f88395c4bc162ccdd1cc7f05b64f449).
Rule: transfer bitstreams as zip or binary, never as text.

## What to do

1. Wait for ZB-00 to go green (SSH into the Mac works, camera works).
2. Confirm the Mac is awake and unlocked, the ZedBoard is powered and
   plugged in, and the board is positioned where the Mac's built-in camera
   can see the LEDs.
3. Copy zedboard_blinky_v3_tieoff.bit to the Mac if it is not already
   there. The Mac currently has only the older e0c5ce0 build,
   `zedboard_blinky_latest.bit`, which predates the tie-off fix. Transfer
   as zip or binary, never as text.
4. Run `openFPGALoader --detect` on the Mac to confirm the board shows up.
5. Flash: `openFPGALoader -b zedboard --write-sram <file.bit>`. Confirm
   done=1.
6. Take two camera photos a second apart and inspect them for LD0 blinking.
7. If LD0 blinks: the clock path is fixed. Report it, then move to the
   t01-t05 isolated tests (ZB-11, ZB-02, ZB-04, ZB-03) and then the
   21-design corpus, shift_chaser first.
8. If LD0 stays dark: the next suspects are BUFGCTRL ZINV polarity
   (hardware validation, see ZB-02) or deeper clock routing. Do not guess;
   report the exact observation (which LEDs are lit or dark in each photo)
   and stop for a decision on the next suspect.

## Files and commands

- Bitstream: ~/workspace/zedboard-hw-loop/bitstreams/zedboard_blinky_v3_tieoff.bit
- Tests: ~/workspace/zedboard-hw-loop/tests/ (t01-t05),
  test_manifest.md, corpus_manifest.md (21 designs).
- Fix code: crates/helion-xilinx7/src/route.rs (route_bufg_tieoffs),
  src/xdevice.rs (DB-driven pip lookup).
- X-Ray DB: ~/workspace/xilinx-dbsrc, zynq7 family; segbits_clk_bufg_bot_r.db.
- Mac folder: ~/Documents/zedboard-helion-debug/ (handoff package, older
  bitstream only).

## Dependencies

Blocked on ZB-00 (Mac access). Validates ZB-02 (BUFG config). Unblocks
ZB-11, ZB-02, ZB-04, ZB-03 (the t01-t05 tests) and the corpus.

## Rules

- No em dashes anywhere.
- Never fabricate terminal output or photos. Report the exact command,
  the exact output, and what the photos actually show.
- LD6 / LD7 may stay dark even on a correct bitstream (RIOB33_SING gap,
  see ZB-09). They are excluded from every pass criterion.
- Do not re-enable the zedboard-hw-test-loop cron.
