# Helion goal: Level 2, "fills the ZedBoard"

Set 2026-11-17. This is the target for every Helion session until it is met.
ZedBoard only; other devices come after.

**Goal:** Helion alone (no Vivado anywhere in the flow) builds designs that use
almost all of the ZedBoard's Zynq-7020, and they are proven correct and seen
working on the board.

## The chip (xc7z020clg484-1, programmable logic)

| Resource | Total | "Filled" target |
|---|---|---|
| LUTs | 53,200 | at least 85% (45,000+) |
| Flip-flops | 106,400 | whatever the filled design needs |
| Block RAM (36 Kb) | 140 | at least 80% (112+) |
| DSP48E1 | 220 | at least 80% (176+) |
| Clock managers (MMCM/PLL) | 4 + 4 | at least one MMCM in use |
| Global clock buffers | 32 | multiple clock domains |

## What counts as done for any rung (all five, no exceptions)

1. **Helion only:** v2 front end, Helion synth, place, route and bitstream. One command reproduces the build.
2. **Logic equal to the RTL:** Verilator co-simulation (svcheck) has 0 mismatches.
3. **Bitstream equal to the RTL:** independent decode plus co-simulation (x7v verify) has 0 mismatches.
4. **Timing real:** Helion's timing report says the target clock is met, and the board agrees (it runs at that clock; where possible, a clock sweep fails where timing predicts).
5. **Works on the board:** it is seen running. Flashing happens only with the owner's go-ahead, watched.

Record for each: build time, LUT/FF/BRAM/DSP use, clock, and the verify results.

## Rung ladder

| # | Rung | Proof on the board | Status |
|---|---|---|---|
| 0 | PicoRV32 SoC at 50 MHz | LEDs count; confirmed by eye | done (needs BTNC; power-on reset in progress) |
| 1 | Ibex routes in under 10 minutes and runs at Vivado's own Fmax (38.9 MHz, ref_ibex; target set 2026-11-17, was 50 MHz) | program runs, LEDs/display show it | **runs on the board at 38.89 MHz** (2026-11-18, LEDs seen counting): `examples/zedboard_ibex_soc.sv`, Ibex + MMCM (VCO 700 MHz / 18) + a ROM program counting on the LEDs; `~/helion-zed-builds/ibex_soc_38_9mhz/`. Helion STA +1.605 ns with `HELION_PLACE_SEED=3` (the default seed reached 36.9 MHz then; at 390833b it meets 38.89 MHz with +4.075 ns, G74 closed, not rebuilt for the board); route 350 s; 6,467 LUTs; x7v verify PASS (20,000 cycles, 0 mismatches; a fast-delay variant passes with the LEDs counting). Not done: a clock sweep on the board, and Helion's STA of its own routes checked against Vivado (G7, G20) |
| 2 | Block RAM: at least 80% of the RAM, with init data | checksum of all RAM contents shown on LEDs | **works on the board at 100 MHz** (2026-11-18, camera-watched): 240 RAMB18 (86%), LD7 done + LD6 all passed + LD0 beat; Helion STA +1.492 ns; `~/helion-zed-builds/bram_fill_100mhz_board/`. The earlier failure was a missing row clock buffer enable (G69, fixed 8a19ba5), not the RAM. x7v verify PASS on this exact bitstream (2026-11-19, md5 42556803...: 4,000 cycles with no reset and with random resets, 0 unexplained bits, 0 mismatches; logs next to it) |
| 3 | DSP: at least 80% of the DSP blocks at 100 MHz or more | known result from a MAC array or FIR shown on LEDs | **works on the board at 100 MHz** (2026-11-18, camera-watched): 192 DSP48E1 (87%, primitive version), LD7 done + LD6 signature passed + LD0 beat; `~/helion-zed-builds/dsp_fill_100mhz/`. Helion STA +0.608 ns, route 348 s, 8,140 LUTs; x7v verify PASS against the behavioural `zedboard_dsp_fill.sv` (6,000 cycles, 0 mismatches). The cause of the old routing failure was a placement collision (G47, fixed 07e3be2). Not done: the plain-SV version (`*` inferred as DSP) on the board |
| 4 | Clocking: MMCM, 2+ clock domains, safe crossings, power-on reset | design runs from MMCM clocks | **works on the board** (2026-11-18, camera-watched, rebuilt at 8a19ba5 with the clock buffer fix; `~/helion-zed-builds/mmcm_board_8a19ba5/`): LD7 (MMCM locked) and LD6 (cross-domain check passed) on in every frame, LD0..LD5 heartbeats and counters changing. One MMCM making 50/125/25 MHz, 3 BUFG; Helion timing met per domain (+15.4/+3.5/+35.7 ns); x7v verify PASS (20k cycles, 0 mismatches). The 2026-11-17 build in `~/helion-zed-builds/mmcm/` predates the fix and has fewer buffer enables: do not flash that one. Individual heartbeat rates not measured from the camera |
| 5 | Board peripherals in the PL: switches, buttons, LEDs, OLED, VGA, Pmods, then HDMI and audio | each one visibly works | LEDs/switches/buttons; VGA and Pmod loopback designs (by the UltraScale+ track, `fm/zedboard-rung5`) build at 100 MHz and pass svcheck; OLED fixed by that track (7e179b3): svcheck, zedbit (+5.0 ns) and x7v over the whole 11.2M-cycle init pass, SSD1306 testbench passes the protocol; VDD/VBAT power-up value still to fix (2026-11-19) |
| 6 | **Fill:** one design at 85%+ LUTs, 80%+ RAM, 80%+ DSP, timing met | it runs and checks itself | **designed and synthesized, not routed yet** (stopped 2026-11-18, the board went away): `examples/zedboard_fill.sv` = the RAM fill + the DSP fill + 1,040 LUT lanes (was 960: the sync reset fix, G59/G46, freed LUTs, d715877); 45,387 LUTs (85.3%), 240 RAMB18 (86%), 192 DSP (87%), 49,972 flops; RTL simulation passes all three self-checks and the wrong-value control fails as it should (`examples/zedboard_fill_expected.py`). Placement works since G75 (23 min, cost 102.6M -> 14.5M) but the placer's estimated worst path is 19 ns, so 100 MHz is unlikely; routing was 9 minutes in (61k of 108k net routes) when stopped. Logs: `~/helion-zed-builds/fill_attempts/`. Next: let run 3's recipe finish (branch fm/g69-g70 at 83387f9), then either meet 100 MHz or build it at a slower clock and say so |
| 7 | ARM side (PS7) talks to the logic over AXI | the ARM reads and writes logic registers | **in progress** (2026-11-19, `fm/rung7-ps7`): PS7 as a hard block in pack/place/route/timing, `examples/zedboard_ps7_axi.sv` AXI3 register block, x7v PS7 boundary, openocd board script (untested on hardware); review and verification running |

## Measured gap to Vivado (2026-11-17, details in ~/helion-zed-builds/vivado_jobs/COMPARISON.md)

- Helion uses **3.2x (Ibex) to 5.1x (PicoRV32) the LUTs** Vivado does for the same RTL. The known biggest cause is register files built from flops: no LUT RAM inference yet.
- Critical paths are about 1.5x longer (Ibex 40 ns vs 26 ns).
- Vivado accepts Helion's placement unchanged (0 cells moved, every design so far). Helion's timing estimate is **not** reliably pessimistic: optimistic on blinky (0.29 ns) and PicoRV32 with the new placer, pessimistic on Ibex (0.76 ns). And every comparison so far used Vivado's routes, so Helion's timing of its own routing is still unvalidated (G7, G20). (Corrected 2026-11-17; an earlier line here said "safe (pessimistic)".)
- Build time: PicoRV32 route is now 34-47 s with PathFinder plus the timing-driven placer (was 577 s; Vivado 23 s). Ibex 242 s on an idle Mac.
- Placement quality: Vivado routing Helion's placement reaches 71% (PicoRV32) and 83% (Ibex) of Vivado's own Fmax with the timing-driven placer (was 64-69%).
- RAM fill (240 RAMB18): Helion builds it end to end, x7v verified. Vivado meets 100 MHz on Helion's own placement (+0.25 ns), but Helion's routing reaches only ~31 MHz (G38, router or wide-net delay model). A 25 MHz version is timing-clean and ready to flash.
- DSP fill (192 DSP48E1, primitives): places, but routing doesn't converge in 1 h (G47). Plain-SV DSP fill needs `*` to DSP inference (G3; without it 1.2 million LUTs).

## State on 2026-11-18 (end of the board session)

- Seen working on the board: rung 1 (Ibex, 38.89 MHz), rung 2 (240 RAMB18, 100 MHz), rung 3 (192 DSP48E1, 100 MHz), rung 4 (MMCM, 3 clocks).
- Merged into `fm/zedboard-merge` (tip 484bd92 at the time): the clock buffer fix (G69), the x7v clock rule (G73), timing-weighted routing (G71), the DSP placement collision fix (G47), the Ibex board design, `HELION_PLACE_SEED`.
- On `fm/g69-g70` only (tip 83387f9), NOT merged: the placer's whole-slice swap (G75), hard-block pins timed in placement, BYP bounce for signal nets, the router's PathFinder handover (G77), `examples/zedboard_fill.sv`. Reason it is not merged: with it a fresh 192-DSP fill build misses 100 MHz by 0.04 to 2.1 ns (G76). It makes Ibex better (default seed +2.3 ns at 38.89 MHz, route 95 s).
- The last outside commit to `fm/zedboard-merge` was at 02:40 on 2026-11-18.

## State on 2026-11-19 (no board, no cluster)

- `fm/zedboard-merge` fast-forwarded to 67843e9 (`fm/g69-g70`); everything listed as unmerged above is now merged. Not pushed.
- Fixed and guarded since then: G76 (critical nets routed first: DSP fill +0.631 ns at 100 MHz), G59/G46 (synchronous reset on the flop's SR pin: rung 4 design 8.6x -> 1.57x Vivado's LUTs, DSP fill 8,140 -> 4,479 LUTs), G58 (MMCM on the CMT nearest its pad), and the benchmark guards G22/G33/G74. None of these bitstreams has been on the board.
- Checked without the board: x7v verify of the MMCM bitstream, svcheck of PicoRV32 SoC / DSP fill / MMCM design against their RTL, all helion-sv and helion-xilinx7 tests, `tools/qor --set full`.
- Database: openXC7/prjxray-db 0c3504f (has spartan7 too) gives byte-identical bitstreams to f4pga master for all six board designs and the four-board counter/PicoRV32 set; x7v passes them all on it (DB.md). G79: the ZedBoard's always-on bits no longer depend on a hand-copied file.
- New gap G78: v2's balanced AND trees keep three equality bit pairs out of one LUT6 (the rest of rung 4's 1.57x).

Later on 2026-11-19:
- Two workflows: rung 7 (above) and G3 (`*` to DSP48E1 in v2, `fm/g3-dsp-v2`: 25x18 products, A/B/M/P registers, accumulate, wide products split; stage 2 restarted after a network drop at 13:06).
- Rung 6 routing (G77): PathFinder does not converge at 1,040 lanes (175,196 overused wires after 9 iterations, 1.4% less per iteration). Router work is running as a third workflow on `fm/g77-pathfinder`.
- A separate track owns UltraScale+ (`fm/ultra-*`) and helps on the ZedBoard with research and new files: the PS7 JTAG fact sheet (`fm/zedboard-ps7-facts`, matches the rung 7 board script), PS7 PL timing (`fm/zedboard-ps7-timing`: no public M_AXI_GP0 setup/clock-to-out numbers), rung 5 designs. That track has no simulator and no fast build; its checks run on the Mac here.

Who does what (2026-11-17):
- **ZedBoard session:** rung 1 (Ibex). Owns the router, timing and device code, the x7v checker core, and in the v2 front end carry chains plus `*` to DSP48E1.
- **Main session:** rungs 2 and 3. Block RAM inference and Xilinx primitive pass-through in v2, BRAM and DSP models in x7v, the fill designs, and the Helion-to-Vivado export (branch `fm/zedboard-l2`).

Rung 6 is the headline: "Helion filled a ZedBoard, end to end, and it works".
Rung 7 is the last piece of "everything on the ZedBoard". Drop it only by the owner's decision.
