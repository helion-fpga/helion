# Pair sweep and random FASM fuzz report (xcku3p, tier 1)

Same-bit feature pairs plus 100 fresh random FASM designs, emitted through helion-bitgen and diffed against the tier-1 reference (public DBs only). Part: xcku3p. Pair sweep: started 2026-11-20T20:33:56, finished 2026-11-20T20:50:08. Fuzz: seed base 20261120, 100 designs, batch runtime 3222.1 seconds.

Emitter: the worktree `helion-bitgen` binary (target/release/helion-bitgen on fm/ultra-bitgen-sweeps, base afa8e911). No emitter code changed in this step; no fixes, no ddmin. No Vivado anywhere; tier 1 (zero diff vs public DBs) is the only bar.

Band bits: the emitter `bits` output excludes the H-1 band (absolute frame bits 1440-1487 of block-0 frames), so band bits never appear in a diff. A feature writing a band bit fails loud at emit time and is bisected to isolate it.

## Pair sweep: per-block coverage

| Block | Agree pairs enumerated | Distinct types | Pairs swept | Green | Red | Contra pairs (instances / types) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| FF_INIT | 226560 | 32 | 128 | 128 | 0 | 238080 / 48 |
| CARRY8 | 169920 | 24 | 96 | 96 | 0 | 216000 / 15 |
| LUTRAM | 184080 | 52 | 104 | 104 | 0 | 187200 / 26 |
| INT_PIP | 200 | 25 | 200 | 200 | 0 | 69976 / 8747 |
| IO_CONFIG | 3976 | 3976 | 119 | 119 | 0 | 33844 / 33844 |
| LUT_INIT | 0 | 0 | 0 | 0 | 0 | 0 / 0 |
| BRAM_CONTENT | 0 | 0 | 0 | 0 | 0 | 0 / 0 |
| BRAM_PARITY | 0 | 0 | 0 | 0 | 0 | 0 / 0 |

Cross-block same-bit pairs: 0 (8 sample slices (one per tile type x Y_ofst 0, 30)).

Coverage notes:
- FF/CARRY8/LUTRAM agree pairs are all the same feature on two slices sharing a column-shared bit (32/24/52 distinct types); the sweep keeps one instance per distinct type per tile type (128/96/104 pairs). Per-slice pairs in these blocks are all contradictory value alternatives (FF 48, CARRY8 15, LUTRAM 26 per slice).
- INT agree pairs are 25 genuine distinct-PIP pairs per tile, kept on the same 8 tiles as the single sweep (200 pairs).
- IO agree pairs are 3976 distinct pairs on the one bridged tile; the sweep keeps one per (shared-position set, site pair), 119 pairs. Packing all 3976 needs ~2800 emits (median conflict degree 2914 of 3976, driven by ~34 constrained bits per feature), about 12 hours of `bits` runs, for no new interaction: a pair's outcome depends only on its shared positions plus the two sites, and every distinct (shared set, site pair) is swept. The 3857 unswept IO pairs are enumerated and counted above, not silently skipped.
- LUT_INIT/BRAM: a LUT holds one 64-bit INIT and a RAMB holds one INIT/INITP value, so two INITs on one BEL/RAMB are contradictory and INITs on different BELs/RAMBs are disjoint (proven); zero testable same-bit pairs.
- Packing rule: two pair items sharing a feature never share an emit (the emitter fails loud on a duplicated FASM line, probed at ultra-bitgen-sweeps-logs/probes/dup-line.fasm).

## Pair sweep: red list

No reds. No global extras.

## Pair sweep: contradictory pairs

Contradictory pairs cannot both hold (some shared position requires different values), so they are counted, never emitted as sweep items. Counts are in the coverage table above (instances / distinct types). A sample of each block's contradictory pairs was emitted alone only to observe what the emitter does; those outcomes are observations, never reds.

Outcome pattern over the 20 probes: 13 fail loud, 7 emit ok. The emitter applies FASM lines in order and validates each feature's must-be-0 bits against the bits set so far: when the 0-requiring feature comes second and the bit is already 1, emit fails loud naming the second feature ("must read 0 but reads 1"); when the 1-requiring feature comes second, it overwrites silently, and in all 7 emit-ok probes the contested position ends at the second line's value (last writer wins, no recheck of the earlier feature). This is order-dependent handling of invalid input, not a tier-1 red: a valid design never has two features disagreeing on a bit. Recorded here so step 4 can bucket it without re-probing.

- CARRY8: `CARRY8:SLICE_X7Y0:CARRY8.CARRY_TYPE.DUAL_CY4 + CARRY8:SLICE_X7Y0:CARRY8.CARRY_TYPE.SINGLE_CY8`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile SLICE_X7Y0 feature CARRY8.CARRY_TYPE.SINGLE_CY8: bit 45 of FAR 0x0000120c must read 0 but reads 1
- CARRY8: `CARRY8:SLICE_X7Y0:CARRY8.CI.AX + CARRY8:SLICE_X7Y0:CARRY8.CI.V0`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile SLICE_X7Y0 feature CARRY8.CI.V0: bit 1 of FAR 0x0000120e must read 0 but reads 1
- CARRY8: `CARRY8:SLICE_X7Y0:CARRY8.CI.AX + CARRY8:SLICE_X7Y0:CARRY8.CI.V1`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile SLICE_X7Y0 feature CARRY8.CI.V1: bit 1 of FAR 0x0000120e must read 0 but reads 1
- CARRY8: `CARRY8:SLICE_X7Y0:CARRY8.CI.V0 + CARRY8:SLICE_X7Y0:CARRY8.CI.V1`: emit ok, bits ok
  - 0x0000120a offset 18: A wants 0, B wants 1, emitter set 1
  - 0x0000120e offset 1: A wants 0, B wants 0, emitter set 0
- FF_INIT: `FF_INIT:SLICE_X7Y0:ABCDFF.CEUSED.V0 + FF_INIT:SLICE_X7Y0:ABCDFF.CEUSED.V1`: emit ok, bits ok
  - 0x0000120e offset 7: A wants 0, B wants 1, emitter set 1
- FF_INIT: `FF_INIT:SLICE_X7Y0:ABCDFF.CLKINV.V0 + FF_INIT:SLICE_X7Y0:ABCDFF.CLKINV.V1`: emit ok, bits ok
  - 0x00001208 offset 19: A wants 0, B wants 1, emitter set 1
- FF_INIT: `FF_INIT:SLICE_X7Y0:ABCDFF.MODE.FF + FF_INIT:SLICE_X7Y0:ABCDFF.MODE.LATCH`: emit ok, bits ok
  - 0x0000120e offset 13: A wants 0, B wants 1, emitter set 1
- FF_INIT: `FF_INIT:SLICE_X7Y0:ABCDFF.SRINV.V0 + FF_INIT:SLICE_X7Y0:ABCDFF.SRINV.V1`: emit ok, bits ok
  - 0x00001209 offset 18: A wants 0, B wants 1, emitter set 1
- INT_PIP: `INT_PIP:INT_X0Y0:PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 + INT_PIP:INT_X0Y0:PIP.BOUNCE_E_0_FT1.INODE_E_5_FT1`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile INT_X0Y0 feature PIP.BOUNCE_E_0_FT1.INODE_E_5_FT1: bit 0 of FAR 0x00000440 must read 0 but reads 1
- INT_PIP: `INT_PIP:INT_X0Y0:PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 + INT_PIP:INT_X0Y0:PIP.BOUNCE_E_0_FT1.INODE_E_9_FT1`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile INT_X0Y0 feature PIP.BOUNCE_E_0_FT1.INODE_E_9_FT1: bit 0 of FAR 0x00000440 must read 0 but reads 1
- INT_PIP: `INT_PIP:INT_X0Y0:PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 + INT_PIP:INT_X0Y0:PIP.BOUNCE_E_0_FT1.INODE_E_BLN_54_FT1`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile INT_X0Y0 feature PIP.BOUNCE_E_0_FT1.INODE_E_BLN_54_FT1: bit 0 of FAR 0x00000440 must read 0 but reads 1
- INT_PIP: `INT_PIP:INT_X0Y0:PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 + INT_PIP:INT_X0Y0:PIP.BOUNCE_E_0_FT1.INODE_E_BLN_58_FT1`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile INT_X0Y0 feature PIP.BOUNCE_E_0_FT1.INODE_E_BLN_58_FT1: bit 0 of FAR 0x00000440 must read 0 but reads 1
- IO_CONFIG: `IO_CONFIG:HPIO_RIGHT_X0Y0:IOB_X0Y0.IOSTANDARD_IN.LVCMOS12 + IO_CONFIG:HPIO_RIGHT_X0Y0:IOB_X0Y0.IOSTANDARD_IN.LVCMOS15`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile HPIO_RIGHT_X0Y0 feature IOB_X0Y0.IOSTANDARD_IN.LVCMOS15: bit 109 of FAR 0x00000002 must read 0 but reads 1
- IO_CONFIG: `IO_CONFIG:HPIO_RIGHT_X0Y0:IOB_X0Y0.IOSTANDARD_IN.LVCMOS12 + IO_CONFIG:HPIO_RIGHT_X0Y0:IOB_X0Y0.IOSTANDARD_IN.LVCMOS18`: emit ok, bits ok
  - frame 2 (tile-relative) offset 46: A wants 0, B wants 0, emitter set 0
  - frame 2 (tile-relative) offset 47: A wants 0, B wants 0, emitter set 0
  - frame 2 (tile-relative) offset 49: A wants 0, B wants 0, emitter set 0
  - frame 2 (tile-relative) offset 50: A wants 0, B wants 0, emitter set 0
  - frame 2 (tile-relative) offset 61: A wants 0, B wants 0, emitter set 0
  - frame 2 (tile-relative) offset 65: A wants 0, B wants 0, emitter set 0
  - frame 2 (tile-relative) offset 73: A wants 1, B wants 1, emitter set 1
  - frame 2 (tile-relative) offset 74: A wants 1, B wants 1, emitter set 1
  - frame 2 (tile-relative) offset 109: A wants 1, B wants 1, emitter set 1
  - frame 2 (tile-relative) offset 110: A wants 0, B wants 0, emitter set 0
  - frame 2 (tile-relative) offset 111: A wants 0, B wants 0, emitter set 0
  - frame 3 (tile-relative) offset 45: A wants 0, B wants 0, emitter set 0
  - frame 3 (tile-relative) offset 46: A wants 0, B wants 0, emitter set 0
  - frame 3 (tile-relative) offset 49: A wants 0, B wants 0, emitter set 0
  - frame 3 (tile-relative) offset 60: A wants 0, B wants 0, emitter set 0
  - frame 3 (tile-relative) offset 65: A wants 0, B wants 0, emitter set 0
  - frame 3 (tile-relative) offset 73: A wants 1, B wants 1, emitter set 1
  - frame 3 (tile-relative) offset 86: A wants 0, B wants 0, emitter set 0
  - frame 3 (tile-relative) offset 109: A wants 0, B wants 1, emitter set 1
  - frame 3 (tile-relative) offset 110: A wants 0, B wants 0, emitter set 0
  - frame 3 (tile-relative) offset 111: A wants 0, B wants 0, emitter set 0
- IO_CONFIG: `IO_CONFIG:HPIO_RIGHT_X0Y0:IOB_X0Y0.IOSTANDARD_IN.LVCMOS12 + IO_CONFIG:HPIO_RIGHT_X0Y0:IOB_X0Y0.IOSTANDARD_IN.POD12_DCI`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile HPIO_RIGHT_X0Y0 feature IOB_X0Y0.IOSTANDARD_IN.POD12_DCI: bit 74 of FAR 0x00000002 must read 0 but reads 1
- IO_CONFIG: `IO_CONFIG:HPIO_RIGHT_X0Y0:IOB_X0Y0.IOSTANDARD_IN.LVCMOS12 + IO_CONFIG:HPIO_RIGHT_X0Y0:IOB_X0Y0.IOSTANDARD_IN.SSTL12_DCI`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile HPIO_RIGHT_X0Y0 feature IOB_X0Y0.IOSTANDARD_IN.SSTL12_DCI: bit 74 of FAR 0x00000002 must read 0 but reads 1
- LUTRAM: `LUTRAM:SLICE_X1Y0:ALUT.DI1.DI1 + LUTRAM:SLICE_X1Y0:ALUT.DI1.SIN`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile SLICE_X1Y0 feature ALUT.DI1.SIN: bit 19 of FAR 0x0000060a must read 0 but reads 1
- LUTRAM: `LUTRAM:SLICE_X1Y0:ALUT.MODE.RAM + LUTRAM:SLICE_X1Y0:ALUT.MODE.LUT`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile SLICE_X1Y0 feature ALUT.MODE.LUT: bit 19 of FAR 0x00000609 must read 0 but reads 1
- LUTRAM: `LUTRAM:SLICE_X1Y0:ALUT.SIZE.LARGE + LUTRAM:SLICE_X1Y0:ALUT.SIZE.SMALL`: emit ok, bits ok
  - 0x00000608 offset 18: A wants 0, B wants 1, emitter set 1
- LUTRAM: `LUTRAM:SLICE_X1Y0:BLUT.DI1.DI1 + LUTRAM:SLICE_X1Y0:BLUT.DI1.SIN`: emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile SLICE_X1Y0 feature BLUT.DI1.SIN: bit 16 of FAR 0x00000609 must read 0 but reads 1

## Pair sweep: blocked inventory

- DSP48E2: no enumerable feature list (prjuray-db site_types documents pins only, BitFiltrator ships no DSP data); unblocker is the Vivado DSP fuzz job.
- URAM288: no enumerable feature list (no public URAM encoding); unblocker is the Vivado URAM fuzz job.
- CLK (CMT/RCLK/BUFGCE/MMCM, INT_INTF, XIPHY, HDIO): no enumerable feature pairs on an emitter-mapped tile; unblocker is the xcku3p tilegrid (CMT_LEFT and BUFGCE/MMCM/PLL site config have no public encoding at all; unblocker is the Vivado clocking fuzz job).
- FFMUX/OUTMUX: emitter `bits` fails loud (no block in its registry); unblocker is an emitter + registry decision in a later step.
- MODE.SRL: block-to-be; emitter `bits` fails loud.
- BRAM config bits: segbits_bram.db is not loaded by the emitter and the harness has no resolver.
- IO_CONFIG on tile instances other than the one bridged HPIO_RIGHT_X0Y0: unblocker is the xcku3p tilegrid.
- IO_CONFIG unswept agree pairs: 3857 of 3976, by the subset rule above (one per shared-position set and site pair); the subset is 119 pairs and every distinct (shared set, site pair) is swept.

## Pair sweep: emits run

Total pair emits: 58 (planned 58). Total seconds (emit+bits): 862.1.
- PAIR-FF_INIT-000: 64 items, 128 lines, emit 0.4s, bits 15.1s, green 64, red 0, extra 0, status ok
- PAIR-FF_INIT-001: 64 items, 128 lines, emit 0.3s, bits 14.6s, green 64, red 0, extra 0, status ok
- PAIR-CARRY8-000: 44 items, 88 lines, emit 0.3s, bits 14.1s, green 44, red 0, extra 0, status ok
- PAIR-CARRY8-001: 44 items, 88 lines, emit 0.3s, bits 14.5s, green 44, red 0, extra 0, status ok
- PAIR-CARRY8-002: 8 items, 16 lines, emit 0.3s, bits 14.0s, green 8, red 0, extra 0, status ok
- PAIR-LUTRAM-000: 52 items, 104 lines, emit 0.3s, bits 14.0s, green 52, red 0, extra 0, status ok
- PAIR-LUTRAM-001: 52 items, 104 lines, emit 0.3s, bits 14.0s, green 52, red 0, extra 0, status ok
- PAIR-INT_PIP-000: 8 items, 16 lines, emit 0.3s, bits 14.3s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-001: 8 items, 16 lines, emit 0.3s, bits 14.8s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-002: 8 items, 16 lines, emit 0.3s, bits 14.6s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-003: 8 items, 16 lines, emit 0.3s, bits 15.1s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-004: 8 items, 16 lines, emit 0.4s, bits 14.5s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-005: 8 items, 16 lines, emit 0.4s, bits 14.4s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-006: 8 items, 16 lines, emit 0.3s, bits 14.3s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-007: 8 items, 16 lines, emit 0.3s, bits 14.6s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-008: 8 items, 16 lines, emit 0.4s, bits 14.3s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-009: 8 items, 16 lines, emit 0.3s, bits 14.1s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-010: 8 items, 16 lines, emit 0.4s, bits 14.7s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-011: 8 items, 16 lines, emit 0.3s, bits 14.5s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-012: 8 items, 16 lines, emit 0.4s, bits 14.5s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-013: 8 items, 16 lines, emit 0.3s, bits 13.8s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-014: 8 items, 16 lines, emit 0.3s, bits 14.3s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-015: 8 items, 16 lines, emit 0.3s, bits 14.2s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-016: 8 items, 16 lines, emit 0.3s, bits 14.8s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-017: 8 items, 16 lines, emit 0.4s, bits 13.9s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-018: 8 items, 16 lines, emit 0.3s, bits 14.3s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-019: 8 items, 16 lines, emit 0.3s, bits 13.9s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-020: 8 items, 16 lines, emit 0.3s, bits 14.6s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-021: 8 items, 16 lines, emit 0.3s, bits 14.6s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-022: 8 items, 16 lines, emit 0.3s, bits 15.4s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-023: 8 items, 16 lines, emit 0.4s, bits 15.0s, green 8, red 0, extra 0, status ok
- PAIR-INT_PIP-024: 8 items, 16 lines, emit 0.3s, bits 14.3s, green 8, red 0, extra 0, status ok
- PAIR-IO_CONFIG-000: 25 items, 50 lines, emit 0.3s, bits 15.8s, green 25, red 0, extra 0, status ok
- PAIR-IO_CONFIG-001: 26 items, 52 lines, emit 0.4s, bits 15.4s, green 26, red 0, extra 0, status ok
- PAIR-IO_CONFIG-002: 25 items, 50 lines, emit 0.3s, bits 15.0s, green 25, red 0, extra 0, status ok
- PAIR-IO_CONFIG-003: 8 items, 16 lines, emit 0.3s, bits 14.7s, green 8, red 0, extra 0, status ok
- PAIR-IO_CONFIG-004: 3 items, 6 lines, emit 0.3s, bits 14.2s, green 3, red 0, extra 0, status ok
- PAIR-IO_CONFIG-005: 2 items, 4 lines, emit 0.3s, bits 13.9s, green 2, red 0, extra 0, status ok
- PAIR-IO_CONFIG-006: 2 items, 4 lines, emit 0.3s, bits 14.3s, green 2, red 0, extra 0, status ok
- PAIR-IO_CONFIG-007: 3 items, 6 lines, emit 0.3s, bits 15.0s, green 3, red 0, extra 0, status ok
- PAIR-IO_CONFIG-008: 2 items, 4 lines, emit 0.3s, bits 14.5s, green 2, red 0, extra 0, status ok
- PAIR-IO_CONFIG-009: 2 items, 4 lines, emit 0.3s, bits 14.7s, green 2, red 0, extra 0, status ok
- PAIR-IO_CONFIG-010: 3 items, 6 lines, emit 0.4s, bits 14.2s, green 3, red 0, extra 0, status ok
- PAIR-IO_CONFIG-011: 1 items, 2 lines, emit 0.3s, bits 14.5s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-012: 2 items, 4 lines, emit 0.4s, bits 14.6s, green 2, red 0, extra 0, status ok
- PAIR-IO_CONFIG-013: 1 items, 2 lines, emit 0.4s, bits 14.0s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-014: 1 items, 2 lines, emit 0.4s, bits 15.2s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-015: 1 items, 2 lines, emit 0.5s, bits 14.3s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-016: 1 items, 2 lines, emit 0.4s, bits 14.8s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-017: 2 items, 4 lines, emit 0.4s, bits 14.5s, green 2, red 0, extra 0, status ok
- PAIR-IO_CONFIG-018: 1 items, 2 lines, emit 0.3s, bits 14.9s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-019: 1 items, 2 lines, emit 0.4s, bits 14.7s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-020: 1 items, 2 lines, emit 0.3s, bits 15.0s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-021: 2 items, 4 lines, emit 0.4s, bits 14.2s, green 2, red 0, extra 0, status ok
- PAIR-IO_CONFIG-022: 1 items, 2 lines, emit 0.3s, bits 14.7s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-023: 1 items, 2 lines, emit 0.3s, bits 14.2s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-024: 1 items, 2 lines, emit 0.3s, bits 14.4s, green 1, red 0, extra 0, status ok
- PAIR-IO_CONFIG-025: 1 items, 2 lines, emit 0.3s, bits 14.5s, green 1, red 0, extra 0, status ok

## Random FASM fuzz

100 fresh random designs from gen_random.py, seed base 20261120 (design seeds 20261120 to 20261219), generated into ~/workspace/ultra-bitgen-sweeps-logs/fuzz-designs and run through the branch-copy harness loop (run_100.py, diff_design per design). Classes alternate bram/core; each design carries random LUT INITs, FF INIT/SRVAL, INT PIPs, and (bram class) random BRAM INIT/INITP, plus the frozen IO lines. CARRY8, LUTRAM mode, and FF control attributes are not generated (covered by the pair sweep); DSP48E2/URAM288/CLK are not generated (no public encoding).

| Block | Green | Red | Blocked |
| --- | ---: | ---: | ---: |
| BRAM_CONTENT | 50 | 0 | 50 |
| BRAM_PARITY | 50 | 0 | 50 |
| CARRY8 | 0 | 0 | 100 |
| CLK | 0 | 0 | 100 |
| DSP48E2 | 0 | 0 | 100 |
| FF_INIT | 96 | 0 | 4 |
| FRAMING | 100 | 0 | 0 |
| INT_PIP | 100 | 0 | 0 |
| IO_CONFIG | 0 | 0 | 100 |
| LUTRAM | 0 | 0 | 100 |
| LUT_INIT | 100 | 0 | 0 |
| URAM288 | 0 | 0 | 100 |

Per-design: 100 designs, mean 32.2 seconds each, batch runtime 3222.1 seconds.
Designs with a red block: 0.

### Fuzz red list

No reds.

### Fuzz blocked records

- BRAM_CONTENT: 50 blocked records
- BRAM_PARITY: 50 blocked records
- CARRY8: 100 blocked records
- CLK: 100 blocked records
- DSP48E2: 100 blocked records
- FF_INIT: 4 blocked records
- IO_CONFIG: 100 blocked records
- LUTRAM: 100 blocked records
- URAM288: 100 blocked records

First blocked record per block (exact missing data):
- LUTRAM, design rand000: no LUTRAM features in golden FASM for rand000/xcku3p; block not exercised by this design
- CARRY8, design rand000: no CARRY8 features in golden FASM for rand000/xcku3p; block not exercised by this design
- IO_CONFIG, design rand000: reference bits are tile-relative; absolute FARs need the xcku3p tilegrid (prjuray-db ships xczu3eg tilegrids only)
- CLK, design rand000: no CLK features in golden FASM for rand000/xcku3p; block not exercised by this design
- DSP48E2, design rand000: no DSP48E2 instances in the golden designs
- URAM288, design rand000: no URAM288 instances in the golden designs
- BRAM_CONTENT, design rand001: no BRAM_CONTENT features in golden FASM for rand001/xcku3p; block not exercised by this design
- BRAM_PARITY, design rand001: no BRAM_PARITY features in golden FASM for rand001/xcku3p; block not exercised by this design
- FF_INIT, design rand030: no FF_INIT features in golden FASM for rand030/xcku3p; block not exercised by this design

## Commands

- `sweep.py --pairs --out ~/workspace/ultra-bitgen-sweeps-logs/pairs-full`
- `python3 gen_random.py --out ~/workspace/ultra-bitgen-sweeps-logs/fuzz-designs --count 100 --seed-base 20261120`
- `run_fuzz_batches.sh`: 4 sequential `run_100.py` batches of 25 (`--start 0/25/50/75 --count 25`, --generated-dir ~/workspace/ultra-bitgen-sweeps-logs/fuzz-designs, --out fuzz-run-b<start>, --emitter-cmd <worktree>/target/release/helion-bitgen), then `merge_fuzz_batches.py` into ~/workspace/ultra-bitgen-sweeps-logs/fuzz-run`

Results: pair sweep.jsonl and pair_summary.json in ~/workspace/ultra-bitgen-sweeps-logs/pairs-full; fuzz diff.jsonl and manifest.json in ~/workspace/ultra-bitgen-sweeps-logs/fuzz-run; fuzz designs in ~/workspace/ultra-bitgen-sweeps-logs/fuzz-designs.
