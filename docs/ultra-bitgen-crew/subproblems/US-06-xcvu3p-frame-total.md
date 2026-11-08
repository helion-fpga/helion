# US-06: xcvu3p frame-total gap (73,035 vs UG570 71,820)

- Track: UltraScale+, bitgen plus device file.
- Branch: fm/ultra-bitstream (device-file lane fm/ultra-bitgen-device,
  33a4b8f). Worktree: ~/workspace/helion-ultra-bitstream.
- Status: OPEN. Flagged provisional, never tuned away.

## Goal

Explain the 1,215-frame gap between our computed xcvu3p frame total and
AMD's published total, and correct the device file if we are wrong.

## Done when

The gap is resolved by Vivado confirmation or by a grounded correction
to the device file, with the reasoning written down. Tuning the number to
match without understanding it is not done.

## What is already done

- The device-file lane delivered (commit 33a4b8f, merge 9289fed, critic
  session 7 PASS with flags): a loader plus demo.py reconstruction
  validation plus a provisional xcvu3p.json.
- The gap: our computed xcvu3p frame total is 73,035 against UG570 Table
  1's 71,820, an overshoot of 1,215. It is flagged provisional and was
  never tuned away.
- The sibling gap: the xcku3p file total is 41,460 frames against UG570
  Table 1's 41,476 for KU3P, a gap of 16, exactly 4 per FAR row.
- CTO slice-count rulings (Nov 7, 2026): xcvu3p adopts 49,260 slices
  (394,080 LUTs, 788,160 FFs); device.rs 49,250 is a rounding artifact
  and must be corrected. xcku3p adopts AMD's 20,340 slices (162,720 LUTs,
  325,440 FFs); the device file's num_slices 27,120 contradicts its own
  num_luts 162,720 and is a file-header anomaly.
- The device.rs grid is INVENTED (144x400, hard-block columns every 12,
  (x+y)%3 SLICEM ratio, bottom rows I/O). Counts are right but it is NOT
  ground truth. Bitgen code must never consume it for FAR mapping.
- The emitter does not wire xcvu3p today; only xcku3p is emitted.

## What was tried and what failed

- The gap was computed and flagged, not resolved. No one has attempted a
  correction, and the number was deliberately not tuned to match.

## What to do

1. Re-derive both totals from the device files and UG570 Table 1, and
   write down exactly where the 1,215 (xcvu3p) and 16 (xcku3p) frames
   come from: which block types, which rows, which columns. If the
   arithmetic shows a miscount in our file (for example a block type
   counted twice, or a row range off by one), correct the file and
   record the correction.
2. Correct the device.rs 49,250 rounding artifact to the ruled 49,260.
3. If the arithmetic does not explain the gap, the only resolution is
   Vivado confirmation (the xcvu3p device file is fuzzing job 1 in the
   Vivado package; see US-08 for the license state). Record the gap as
   blocked on that confirmation, with the exact numbers.
4. Re-run the device-file validation (demo.py reconstruction) and the
   full test suite after any correction. 153 tests must still pass.

## Files and commands

- Device files: crates/helion-ultrascale/data/ (vendored),
  plus the loader in crates/helion-ultrascale/src/device_file.rs.
- Contracts: docs/ultra-bitgen-crew/INTERFACES.md (device file schema;
  every provisional xcvu3p value flagged `provisional: true`).
- Reference: UG570 Table 1 (frame totals per part).
- Fuzzing package: ~/workspace/your_files/vivado-fuzzing-instructions.md
  (job 1 is the xcvu3p device file).

## Dependencies

Blocked on Vivado confirmation if the arithmetic does not explain the
gap. Related to US-01 (the tilegrid) and US-08 (tier 2).

## Rules

- No em dashes anywhere.
- Never tune a number to match without understanding it. A provisional
  value stays flagged until real data replaces it.
- Bitgen code must never consume the invented device.rs grid for FAR
  mapping.
