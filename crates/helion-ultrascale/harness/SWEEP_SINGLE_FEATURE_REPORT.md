# Single-feature sweep report (xcku3p, tier 1)

Sweep of every tile instance x every feature from the public DBs, one feature per emit slot, packed onto disjoint bit positions. Part: xcku3p. Started: 2026-11-20T18:32:53. Finished: 2026-11-20T19:40:47.

Emitter: the worktree `helion-bitgen` binary (target/release/helion-bitgen on fm/ultra-bitgen-sweeps, base afa8e911). No emitter code changed in this step; this is mechanics plus measurement. No Vivado anywhere; tier 1 (zero diff vs public DBs) is the only bar.

Band bits: the emitter `bits` output excludes the H-1 band (absolute frame bits 1440-1487 of block-0 frames), so band bits never appear in a diff. A feature writing a band bit fails loud at emit time and is bisected to isolate it.

## Per-block coverage

| Block | Features in universe | Items swept | Green | Red | Blocked | Coverage |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| LUT_INIT | 13885440 | 1301760 | 1301760 | 0 | 0 (+0 inventoried) | 100.00% (13885440/13885440 features green) |
| FF_INIT | 2603520 | 2603520 | 2603520 | 0 | 0 (+542 inventoried) | 100.00% (2603520/2603520 features green) |
| CARRY8 | 650880 | 650880 | 650880 | 0 | 0 (+0 inventoried) | 100.00% (650880/650880 features green) |
| BRAM_CONTENT | 15728640 | 13440 | 13440 | 0 | 0 (+270 inventoried) | 100.00% (15728640/15728640 features green) |
| BRAM_PARITY | 1966080 | 10560 | 10560 | 0 | 0 (+0 inventoried) | 100.00% (1966080/1966080 features green) |
| INT_PIP | 57336 | 57336 | 57336 | 0 | 0 (+0 inventoried) | 100.00% (57336/57336 features green) |
| LUTRAM | 648960 | 648960 | 648960 | 0 | 0 (+99840 inventoried) | 100.00% (648960/648960 features green) |
| IO_CONFIG | 1560 | 1560 | 1560 | 0 | 0 (+40640 inventoried) | 100.00% (1560/1560 features green) |
| CLK | 0 | 0 | 0 | 0 | 0 (+7270 inventoried) | not swept |
| DSP48E2 | 0 | 0 | 0 | 0 | 0 (+0 inventoried) | not swept |
| URAM288 | 0 | 0 | 0 | 0 | 0 (+0 inventoried) | not swept |

Coverage notes:
- LUT_INIT, BRAM_CONTENT, BRAM_PARITY coverage is per bit index (binary-code scheme): a feature is one bit position of one BEL / RAMB.
- INT_PIP coverage is per (tile, PIP) pair in the reduced sweep: every PIP on 8 tiles plus 1 PIP on every INT tile. The full 102,459,360-pair cross product is not enumerated; the encoding is tile-independent by DB construction (one segbits_int.db) and every tile's base FAR is tested by the tile sweep.
- IO_CONFIG coverage is tile-relative (provisional HPIO base FAR 0x000) on the one bridged tile; the absolute placement stays blocked on the tilegrid.

## Red list

No reds. No global extras.
## Blocked inventory

- FF_INIT: 542: FFMUX/OUTMUX lines in the CLB segbits DBs (96 FFMUX + 39 OUTMUX per tile type x 4 tile types x 27120 slices): emitter `bits` fails loud (no block in its registry) and the harness registry has no block; needs an emitter + registry decision in a later step
- LUTRAM: 99840: MODE.SRL lines (8 per CLEM/CLEM_R slice): block-to-be; emitter `bits` fails loud on them
- BRAM_CONTENT: 270: BRAM config lines in segbits_bram.db: the emitter does not load segbits_bram.db and the harness has no resolver; the 36,864 segbits_bram.block_ram.db lines are the same content/parity positions swept via BitFiltrator, not a separate universe
- IO_CONFIG: 1640: HDIO features (812 segbits_hdio_top_right.db + 828 segbits_hdio_bot_right.db): no HDIO tile mapping; unblocker is the xcku3p tilegrid
- IO_CONFIG: 39000: HPIO features on tile instances other than the one bridged HPIO_RIGHT_X0Y0 (26 IOB sites x 59-60 features is the per-tile count; the tile-instance count is unknown without a tilegrid): unblocker is the xcku3p tilegrid
- CLK: 104: CMT_RIGHT features (segbits_cmt_right.db): tile-relative, no CMT tile mapping; unblocker is the xcku3p tilegrid
- CLK: 0: CMT_LEFT: no segbits file exists in prjuray-db at all; no enumerable features; unblocker is Vivado clocking fuzzing
- CLK: 5360: RCLK features across the 15 segbits_rclk_*.db files: tile-relative, no RCLK tile mapping and the emitter loads no rclk DBs; unblocker is the xcku3p tilegrid
- CLK: 50: INT_INTF_LEFT_TERM_PSS features (segbits_int_intf_left_term_pss.db): no INT_INTF tile mapping; unblocker is the xcku3p tilegrid
- CLK: 1756: XIPHY_BYTE_RIGHT features (segbits_xiphy_byte_right.db): no XIPHY tile mapping; unblocker is the xcku3p tilegrid
- DSP48E2: 0: no enumerable feature list: prjuray-db site_types documents DSP48E2 pins only, BitFiltrator ships no DSP data; unblocker is the Vivado DSP fuzz job
- URAM288: 0: no enumerable feature list: no public URAM encoding; unblocker is the Vivado URAM fuzz job
- CLK: 0: BUFGCE/MMCM/PLL site config: no public encoding in either source; unblocker is the Vivado clocking fuzz job

## Probes (blocked classes, exact emitter outcome)

- dsp (`DSP48E2_X0Y0.AREG.0`): emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile DSP48E2_X0Y0 feature AREG.0: DSP48E2 config-bit encoding is missing, no public source documents DSP48E2 config bits (prjuray-db ships no segbits_dsp DB; BitFiltrator ULTRASCALE_PLUS.json has no DSP table); blocked on Vivado fuzzing job 2 (DSP48E2 attribute sweep); refusing to emit guessed bits
- uram (`URAM288_X0Y0.MEM.0`): emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile URAM288_X0Y0 feature MEM.0: URAM288 encoding is missing from every public source (no URAM288 content or config encoding in prjuray-db or BitFiltrator ULTRASCALE_PLUS.json); blocked until the Vivado fuzzing job (Job 5) lands it, no bits emitted
- clk-cmt (`CMT_RIGHT_X0Y0.PIP.CLK_HROUTE_X0Y0.CLK_HROUTE_X0Y0`): emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: unknown tile CMT_RIGHT_X0Y0
- clk-rclk (`RCLK_INT_L_X0Y0.BUFCE_LEAF_X0Y0.BUF`): emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: unknown tile RCLK_INT_L_X0Y0
- bram-config (`RAMB18_X0Y0.BRAM.RAMB18E2_L.CLKARDCLKINV.V0`): emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: tile RAMB18_X0Y0: unknown feature BRAM.RAMB18E2_L.CLKARDCLKINV.V0
- hdio (`HDIO_TOP_RIGHT_X0Y0.IOB_X0Y0.IOSTANDARD_IN.LVCMOS12`): emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: unknown tile HDIO_TOP_RIGHT_X0Y0
- xiphy (`XIPHY_BYTE_RIGHT_X0Y0.BITSLICE_RX_TX_X0Y0.BYPASS.RX_Q5.V0`): emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: unknown tile XIPHY_BYTE_RIGHT_X0Y0
- int-intf (`INT_INTF_LEFT_TERM_PSS_X0Y0.OUTPUTS_ENABLED.0`): emit failed loud: emit failed (exit 1): helion-bitgen: emit failed: unknown tile INT_INTF_LEFT_TERM_PSS_X0Y0
- ffmux (`SLICE_X0Y0.FFMUXA1.BYP`): emit ok, bits failed loud: bits failed (exit 1): helion-bitgen: parse-back feature SLICE_X0Y0.FFMUXA1.BYP matches no block in the registry
- srl (`SLICE_X1Y0.ALUT.MODE.SRL`): emit ok, bits failed loud: bits failed (exit 1): helion-bitgen: parse-back feature SLICE_X1Y0.ALUT.MODE.SRL matches no block in the registry

## Emits run

Total emits: 153. Total seconds (emit+bits): 2917.4.
- LUT_INIT-000: 200000 items, 200000 lines, emit 5.3s, bits 24.9s, green 200000, red 0, extra 0, status ok
- LUT_INIT-001: 200000 items, 200000 lines, emit 5.0s, bits 24.9s, green 200000, red 0, extra 0, status ok
- LUT_INIT-002: 200000 items, 200000 lines, emit 5.0s, bits 25.4s, green 200000, red 0, extra 0, status ok
- LUT_INIT-003: 200000 items, 200000 lines, emit 5.2s, bits 23.9s, green 200000, red 0, extra 0, status ok
- LUT_INIT-004: 200000 items, 200000 lines, emit 4.8s, bits 24.9s, green 200000, red 0, extra 0, status ok
- LUT_INIT-005: 200000 items, 200000 lines, emit 4.8s, bits 25.4s, green 200000, red 0, extra 0, status ok
- LUT_INIT-006: 101760 items, 101760 lines, emit 2.4s, bits 21.2s, green 101760, red 0, extra 0, status ok
- FF_INIT-000: 200000 items, 200000 lines, emit 1.0s, bits 16.4s, green 200000, red 0, extra 0, status ok
- FF_INIT-001: 200000 items, 200000 lines, emit 0.9s, bits 15.7s, green 200000, red 0, extra 0, status ok
- FF_INIT-002: 200000 items, 200000 lines, emit 1.0s, bits 15.3s, green 200000, red 0, extra 0, status ok
- FF_INIT-003: 200000 items, 200000 lines, emit 0.9s, bits 16.0s, green 200000, red 0, extra 0, status ok
- FF_INIT-004: 200000 items, 200000 lines, emit 1.1s, bits 17.2s, green 200000, red 0, extra 0, status ok
- FF_INIT-005: 200000 items, 200000 lines, emit 1.1s, bits 16.4s, green 200000, red 0, extra 0, status ok
- FF_INIT-006: 200000 items, 200000 lines, emit 1.0s, bits 15.1s, green 200000, red 0, extra 0, status ok
- FF_INIT-007: 200000 items, 200000 lines, emit 1.1s, bits 15.4s, green 200000, red 0, extra 0, status ok
- FF_INIT-008: 200000 items, 200000 lines, emit 1.0s, bits 15.3s, green 200000, red 0, extra 0, status ok
- FF_INIT-009: 200000 items, 200000 lines, emit 1.0s, bits 15.6s, green 200000, red 0, extra 0, status ok
- FF_INIT-010: 200000 items, 200000 lines, emit 1.0s, bits 17.0s, green 200000, red 0, extra 0, status ok
- FF_INIT-011: 200000 items, 200000 lines, emit 1.0s, bits 16.0s, green 200000, red 0, extra 0, status ok
- FF_INIT-012: 101760 items, 101760 lines, emit 0.8s, bits 15.8s, green 101760, red 0, extra 0, status ok
- FF_INIT-013: 101760 items, 101760 lines, emit 0.7s, bits 15.3s, green 101760, red 0, extra 0, status ok
- CARRY8-000: 200000 items, 200000 lines, emit 0.8s, bits 16.0s, green 200000, red 0, extra 0, status ok
- CARRY8-001: 200000 items, 200000 lines, emit 0.8s, bits 15.9s, green 200000, red 0, extra 0, status ok
- CARRY8-002: 134684 items, 134684 lines, emit 0.7s, bits 15.1s, green 134684, red 0, extra 0, status ok
- CARRY8-003: 98320 items, 98320 lines, emit 0.5s, bits 15.4s, green 98320, red 0, extra 0, status ok
- CARRY8-004: 17876 items, 17876 lines, emit 0.4s, bits 15.6s, green 17876, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis0-bis0-bis0-bis0: 840 items, 840 lines, emit 4.2s, bits 31.9s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis0-bis0-bis0-bis1: 840 items, 840 lines, emit 4.3s, bits 32.3s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis0-bis0-bis1-bis0: 840 items, 840 lines, emit 4.2s, bits 30.2s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis0-bis0-bis1-bis1: 840 items, 840 lines, emit 4.0s, bits 30.6s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis0-bis1-bis0-bis0: 840 items, 840 lines, emit 4.5s, bits 30.5s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis0-bis1-bis0-bis1: 840 items, 840 lines, emit 4.2s, bits 32.3s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis0-bis1-bis1-bis0: 840 items, 840 lines, emit 4.3s, bits 32.0s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis0-bis1-bis1-bis1: 840 items, 840 lines, emit 4.4s, bits 31.7s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis1-bis0-bis0-bis0: 840 items, 840 lines, emit 4.6s, bits 31.3s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis1-bis0-bis0-bis1: 840 items, 840 lines, emit 5.0s, bits 32.0s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis1-bis0-bis1-bis0: 840 items, 840 lines, emit 4.6s, bits 31.6s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis1-bis0-bis1-bis1: 840 items, 840 lines, emit 4.1s, bits 30.4s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis1-bis1-bis0-bis0: 840 items, 840 lines, emit 3.9s, bits 30.1s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis1-bis1-bis0-bis1: 840 items, 840 lines, emit 4.4s, bits 29.7s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis1-bis1-bis1-bis0: 840 items, 840 lines, emit 4.3s, bits 31.0s, green 840, red 0, extra 0, status ok
- BRAM_CONTENT-000-bis1-bis1-bis1-bis1: 840 items, 840 lines, emit 4.1s, bits 31.0s, green 840, red 0, extra 0, status ok
- BRAM_PARITY-000-bis0-bis0-bis0-bis0: 660 items, 660 lines, emit 0.7s, bits 20.0s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis0-bis0-bis0-bis1: 660 items, 660 lines, emit 0.7s, bits 22.8s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis0-bis0-bis1-bis0: 660 items, 660 lines, emit 0.8s, bits 22.6s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis0-bis0-bis1-bis1: 660 items, 660 lines, emit 0.7s, bits 20.2s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis0-bis1-bis0-bis0: 660 items, 660 lines, emit 0.6s, bits 21.0s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis0-bis1-bis0-bis1: 660 items, 660 lines, emit 0.6s, bits 24.0s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis0-bis1-bis1-bis0: 660 items, 660 lines, emit 0.6s, bits 23.4s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis0-bis1-bis1-bis1: 660 items, 660 lines, emit 0.6s, bits 20.4s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis1-bis0-bis0-bis0: 660 items, 660 lines, emit 0.7s, bits 19.9s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis1-bis0-bis0-bis1: 660 items, 660 lines, emit 0.6s, bits 22.8s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis1-bis0-bis1-bis0: 660 items, 660 lines, emit 0.6s, bits 23.7s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis1-bis0-bis1-bis1: 660 items, 660 lines, emit 0.7s, bits 20.4s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis1-bis1-bis0-bis0: 660 items, 660 lines, emit 0.7s, bits 19.5s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis1-bis1-bis0-bis1: 660 items, 660 lines, emit 0.6s, bits 22.7s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis1-bis1-bis1-bis0: 660 items, 660 lines, emit 0.6s, bits 23.4s, green 660, red 0, extra 0, status ok
- BRAM_PARITY-000-bis1-bis1-bis1-bis1: 660 items, 660 lines, emit 0.7s, bits 20.5s, green 660, red 0, extra 0, status ok
- INT_PIP-000: 32920 items, 32920 lines, emit 0.5s, bits 15.3s, green 32920, red 0, extra 0, status ok
- INT_PIP-001: 5808 items, 5808 lines, emit 0.4s, bits 15.4s, green 5808, red 0, extra 0, status ok
- INT_PIP-002: 5776 items, 5776 lines, emit 0.3s, bits 15.5s, green 5776, red 0, extra 0, status ok
- INT_PIP-003: 5768 items, 5768 lines, emit 0.3s, bits 15.9s, green 5768, red 0, extra 0, status ok
- INT_PIP-004: 2704 items, 2704 lines, emit 0.4s, bits 16.1s, green 2704, red 0, extra 0, status ok
- INT_PIP-005: 2416 items, 2416 lines, emit 0.4s, bits 16.0s, green 2416, red 0, extra 0, status ok
- INT_PIP-006: 1392 items, 1392 lines, emit 0.3s, bits 15.3s, green 1392, red 0, extra 0, status ok
- INT_PIP-007: 400 items, 400 lines, emit 0.3s, bits 15.6s, green 400, red 0, extra 0, status ok
- INT_PIP-008: 152 items, 152 lines, emit 0.3s, bits 15.3s, green 152, red 0, extra 0, status ok
- LUTRAM-000: 200000 items, 200000 lines, emit 0.8s, bits 16.1s, green 200000, red 0, extra 0, status ok
- LUTRAM-001: 200000 items, 200000 lines, emit 0.8s, bits 14.9s, green 200000, red 0, extra 0, status ok
- LUTRAM-002: 124483 items, 124483 lines, emit 0.7s, bits 15.0s, green 124483, red 0, extra 0, status ok
- LUTRAM-003: 124477 items, 124477 lines, emit 0.6s, bits 15.1s, green 124477, red 0, extra 0, status ok
- IO_CONFIG-000: 142 items, 142 lines, emit 0.3s, bits 14.7s, green 142, red 0, extra 0, status ok
- IO_CONFIG-001: 78 items, 78 lines, emit 0.3s, bits 14.6s, green 78, red 0, extra 0, status ok
- IO_CONFIG-002: 78 items, 78 lines, emit 0.3s, bits 15.2s, green 78, red 0, extra 0, status ok
- IO_CONFIG-003: 77 items, 77 lines, emit 0.3s, bits 14.9s, green 77, red 0, extra 0, status ok
- IO_CONFIG-004: 50 items, 50 lines, emit 0.3s, bits 15.4s, green 50, red 0, extra 0, status ok
- IO_CONFIG-005: 49 items, 49 lines, emit 0.3s, bits 15.0s, green 49, red 0, extra 0, status ok
- IO_CONFIG-006: 50 items, 50 lines, emit 0.3s, bits 15.5s, green 50, red 0, extra 0, status ok
- IO_CONFIG-007: 24 items, 24 lines, emit 0.3s, bits 15.0s, green 24, red 0, extra 0, status ok
- IO_CONFIG-008: 25 items, 25 lines, emit 0.3s, bits 15.3s, green 25, red 0, extra 0, status ok
- IO_CONFIG-009: 25 items, 25 lines, emit 0.3s, bits 15.5s, green 25, red 0, extra 0, status ok
- IO_CONFIG-010: 25 items, 25 lines, emit 0.3s, bits 15.3s, green 25, red 0, extra 0, status ok
- IO_CONFIG-011: 23 items, 23 lines, emit 0.3s, bits 14.9s, green 23, red 0, extra 0, status ok
- IO_CONFIG-012: 24 items, 24 lines, emit 0.3s, bits 15.2s, green 24, red 0, extra 0, status ok
- IO_CONFIG-013: 25 items, 25 lines, emit 0.3s, bits 15.1s, green 25, red 0, extra 0, status ok
- IO_CONFIG-014: 25 items, 25 lines, emit 0.3s, bits 14.5s, green 25, red 0, extra 0, status ok
- IO_CONFIG-015: 25 items, 25 lines, emit 0.3s, bits 15.5s, green 25, red 0, extra 0, status ok
- IO_CONFIG-016: 24 items, 24 lines, emit 0.3s, bits 14.6s, green 24, red 0, extra 0, status ok
- IO_CONFIG-017: 24 items, 24 lines, emit 0.3s, bits 15.2s, green 24, red 0, extra 0, status ok
- IO_CONFIG-018: 24 items, 24 lines, emit 0.3s, bits 15.1s, green 24, red 0, extra 0, status ok
- IO_CONFIG-019: 24 items, 24 lines, emit 0.3s, bits 15.7s, green 24, red 0, extra 0, status ok
- IO_CONFIG-020: 24 items, 24 lines, emit 0.3s, bits 15.2s, green 24, red 0, extra 0, status ok
- IO_CONFIG-021: 24 items, 24 lines, emit 0.3s, bits 14.8s, green 24, red 0, extra 0, status ok
- IO_CONFIG-022: 24 items, 24 lines, emit 0.4s, bits 15.3s, green 24, red 0, extra 0, status ok
- IO_CONFIG-023: 24 items, 24 lines, emit 0.4s, bits 14.5s, green 24, red 0, extra 0, status ok
- IO_CONFIG-024: 24 items, 24 lines, emit 0.3s, bits 14.7s, green 24, red 0, extra 0, status ok
- IO_CONFIG-025: 24 items, 24 lines, emit 0.3s, bits 14.5s, green 24, red 0, extra 0, status ok
- IO_CONFIG-026: 24 items, 24 lines, emit 0.3s, bits 14.8s, green 24, red 0, extra 0, status ok
- IO_CONFIG-027: 24 items, 24 lines, emit 0.4s, bits 14.7s, green 24, red 0, extra 0, status ok
- IO_CONFIG-028: 25 items, 25 lines, emit 0.3s, bits 15.1s, green 25, red 0, extra 0, status ok
- IO_CONFIG-029: 25 items, 25 lines, emit 0.4s, bits 14.8s, green 25, red 0, extra 0, status ok
- IO_CONFIG-030: 25 items, 25 lines, emit 0.3s, bits 14.9s, green 25, red 0, extra 0, status ok
- IO_CONFIG-031: 25 items, 25 lines, emit 0.3s, bits 15.3s, green 25, red 0, extra 0, status ok
- IO_CONFIG-032: 24 items, 24 lines, emit 0.4s, bits 15.0s, green 24, red 0, extra 0, status ok
- IO_CONFIG-033: 24 items, 24 lines, emit 0.4s, bits 14.8s, green 24, red 0, extra 0, status ok
- IO_CONFIG-034: 24 items, 24 lines, emit 0.4s, bits 15.3s, green 24, red 0, extra 0, status ok
- IO_CONFIG-035: 25 items, 25 lines, emit 0.4s, bits 14.9s, green 25, red 0, extra 0, status ok
- IO_CONFIG-036: 24 items, 24 lines, emit 0.4s, bits 14.8s, green 24, red 0, extra 0, status ok
- IO_CONFIG-037: 23 items, 23 lines, emit 0.4s, bits 15.3s, green 23, red 0, extra 0, status ok
- IO_CONFIG-038: 23 items, 23 lines, emit 0.4s, bits 16.1s, green 23, red 0, extra 0, status ok
- IO_CONFIG-039: 23 items, 23 lines, emit 0.3s, bits 14.7s, green 23, red 0, extra 0, status ok
- IO_CONFIG-040: 23 items, 23 lines, emit 0.3s, bits 14.5s, green 23, red 0, extra 0, status ok
- IO_CONFIG-041: 23 items, 23 lines, emit 0.3s, bits 14.8s, green 23, red 0, extra 0, status ok
- IO_CONFIG-042: 23 items, 23 lines, emit 0.3s, bits 14.3s, green 23, red 0, extra 0, status ok
- IO_CONFIG-043: 24 items, 24 lines, emit 0.3s, bits 14.0s, green 24, red 0, extra 0, status ok
- IO_CONFIG-044: 24 items, 24 lines, emit 0.3s, bits 15.1s, green 24, red 0, extra 0, status ok
- IO_CONFIG-045: 24 items, 24 lines, emit 0.3s, bits 14.3s, green 24, red 0, extra 0, status ok
- IO_CONFIG-046: 23 items, 23 lines, emit 0.4s, bits 15.0s, green 23, red 0, extra 0, status ok
- IO_CONFIG-047: 3 items, 3 lines, emit 0.3s, bits 15.5s, green 3, red 0, extra 0, status ok
- IO_CONFIG-048: 4 items, 4 lines, emit 0.3s, bits 14.9s, green 4, red 0, extra 0, status ok
- IO_CONFIG-049: 3 items, 3 lines, emit 0.3s, bits 14.3s, green 3, red 0, extra 0, status ok
- IO_CONFIG-050: 2 items, 2 lines, emit 0.3s, bits 14.9s, green 2, red 0, extra 0, status ok
- IO_CONFIG-051: 2 items, 2 lines, emit 0.4s, bits 14.7s, green 2, red 0, extra 0, status ok
- IO_CONFIG-052: 2 items, 2 lines, emit 0.3s, bits 14.2s, green 2, red 0, extra 0, status ok
- IO_CONFIG-053: 2 items, 2 lines, emit 0.3s, bits 14.7s, green 2, red 0, extra 0, status ok
- IO_CONFIG-054: 2 items, 2 lines, emit 0.4s, bits 15.3s, green 2, red 0, extra 0, status ok
- IO_CONFIG-055: 2 items, 2 lines, emit 0.3s, bits 15.3s, green 2, red 0, extra 0, status ok
- IO_CONFIG-056: 2 items, 2 lines, emit 0.4s, bits 14.7s, green 2, red 0, extra 0, status ok
- IO_CONFIG-057: 2 items, 2 lines, emit 0.3s, bits 14.3s, green 2, red 0, extra 0, status ok
- IO_CONFIG-058: 2 items, 2 lines, emit 0.3s, bits 13.9s, green 2, red 0, extra 0, status ok
- IO_CONFIG-059: 2 items, 2 lines, emit 0.3s, bits 14.7s, green 2, red 0, extra 0, status ok
- IO_CONFIG-060: 2 items, 2 lines, emit 0.3s, bits 14.5s, green 2, red 0, extra 0, status ok
- IO_CONFIG-061: 2 items, 2 lines, emit 0.3s, bits 20.6s, green 2, red 0, extra 0, status ok
- IO_CONFIG-062: 2 items, 2 lines, emit 0.5s, bits 14.9s, green 2, red 0, extra 0, status ok
- IO_CONFIG-063: 2 items, 2 lines, emit 0.3s, bits 14.0s, green 2, red 0, extra 0, status ok
- IO_CONFIG-064: 2 items, 2 lines, emit 0.3s, bits 14.2s, green 2, red 0, extra 0, status ok
- IO_CONFIG-065: 2 items, 2 lines, emit 0.4s, bits 17.6s, green 2, red 0, extra 0, status ok
- IO_CONFIG-066: 2 items, 2 lines, emit 0.4s, bits 22.4s, green 2, red 0, extra 0, status ok
- IO_CONFIG-067: 2 items, 2 lines, emit 0.8s, bits 24.4s, green 2, red 0, extra 0, status ok
- IO_CONFIG-068: 2 items, 2 lines, emit 0.3s, bits 13.8s, green 2, red 0, extra 0, status ok
- IO_CONFIG-069: 2 items, 2 lines, emit 0.3s, bits 14.6s, green 2, red 0, extra 0, status ok
- IO_CONFIG-070: 2 items, 2 lines, emit 0.3s, bits 14.0s, green 2, red 0, extra 0, status ok
- IO_CONFIG-071: 2 items, 2 lines, emit 0.3s, bits 14.1s, green 2, red 0, extra 0, status ok
- IO_CONFIG-072: 2 items, 2 lines, emit 0.3s, bits 13.8s, green 2, red 0, extra 0, status ok
- IO_CONFIG-073: 2 items, 2 lines, emit 0.3s, bits 14.2s, green 2, red 0, extra 0, status ok
- IO_CONFIG-074: 2 items, 2 lines, emit 0.3s, bits 13.8s, green 2, red 0, extra 0, status ok
- IO_CONFIG-075: 2 items, 2 lines, emit 0.3s, bits 14.0s, green 2, red 0, extra 0, status ok
- IO_CONFIG-076: 2 items, 2 lines, emit 0.3s, bits 14.1s, green 2, red 0, extra 0, status ok
- IO_CONFIG-077: 2 items, 2 lines, emit 0.3s, bits 14.1s, green 2, red 0, extra 0, status ok
- IO_CONFIG-078: 2 items, 2 lines, emit 0.3s, bits 15.2s, green 2, red 0, extra 0, status ok
- IO_CONFIG-079: 2 items, 2 lines, emit 0.3s, bits 13.7s, green 2, red 0, extra 0, status ok
- IO_CONFIG-080: 2 items, 2 lines, emit 0.3s, bits 14.1s, green 2, red 0, extra 0, status ok
- IO_CONFIG-081: 1 items, 1 lines, emit 0.4s, bits 14.2s, green 1, red 0, extra 0, status ok

## Commands

- `sweep.py --out ~/workspace/ultra-bitgen-sweeps-logs/sweeps-full`

Results: sweep.jsonl and sweep_summary.json in ~/workspace/ultra-bitgen-sweeps-logs/sweeps-full. Per-emit FASM files are kept in the same directory; .bit files are deleted after diffing (the FASM plus the emitter reproduces them); .bits files are kept only for emits with reds or extras.
