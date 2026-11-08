# Helion 7-Series Backend: ZedBoard Debug Handoff

> STALE, Oct 22, 2026. Superseded by HANDOFF.md (the fresh combined
> handoff) and HANDOFF-INDEX.md (the master map). Kept for history only.
> In particular the BUFG section below describes the 10-bit config as a
> guess; the corrected 7-feature config and the BUFG tie-off fix landed
> after this file was written.

## The problem

Helion generates bitstreams for the ZedBoard (Xilinx xc7z020) that program successfully (openFPGALoader reports done=1) but the designs silently fail to function. LEDs do not blink. A Vivado-generated bitstream for the same board works fine, so the hardware and JTAG chain are good.

## Hardware

- Board: Digilent ZedBoard
- FPGA: xc7z020clg484-1 (Zynq-7000, 7-series fabric)
- Clock: 100MHz on pin Y9
- LEDs: 8, pins T22/T21/U22/U21/V22/W22/U19/U14 (see tests/zedboard.xdc)
- Buttons: 5, pins P16/R16/N15/R18/T18
- Programmer: openFPGALoader 1.2.0 (Homebrew)
- Program command: `openFPGALoader -b zedboard --write-sram <file.bit>`

## Repository

- Helion source: https://github.com/helion-fpga/helion
- The 7-series backend lives in `crates/helion-xilinx7/` (snapshot in `code/`)
- Key file: `code/emit.rs` (FASM generation, this is where most bugs are)
- X-Ray database: Project X-Ray (f4pga/prjxray), zynq7 family
- Relevant DB files in `xray-db/`: segbits_clk_bufg_bot_r.db (BUFGCTRL bits), segbits_clbll_l.db (CLB bits incl. CARRY4, FF controls)

## What was found (Oct 22, 2026)

### 1. BUFGCTRL clock buffer: incomplete, then guessed (STILL BROKEN)

`emit.rs` originally emitted only 2 of 10 required FASM features for BUFGCTRL (IN_USE + IS_IGNORE0_INVERTED). The clock was marked used but never configured to pass, so no flip-flop ever saw a clock edge.

Fix attempt: emit all 10 features from the X-Ray DB:
- IN_USE, INIT_OUT, IS_IGNORE0_INVERTED, IS_IGNORE1_INVERTED, PRESELECT_I1
- ZINV_CE0, ZINV_CE1, ZINV_S0, ZINV_S1, ZPRESELECT_I0

Problem: the ZINV_ bit polarities are a GUESS, never validated against hardware or docs. The v2 bitstream with all 10 bits still does not blink. The BUFG worker is investigating whether the polarity is inverted or whether additional clock-routing bits are needed.

See: `code/emit.rs` lines ~163-178, `xray-db/segbits_clk_bufg_bot_r.db`

### 2. CARRY4: dead code (NOT the blinky bug)

`emit.rs` emitted `CARRY4.IN_USE`, a feature that does not exist in the X-Ray DB. However, investigation proved the SV frontend (`crates/helion-sv`) never creates Carry4 cells at all (grep returns 0 hits). The blinky's 27-bit counter is built from LUTs, not carry chains. The CARRY4 emit code never executes. Changed to emit valid ACY0/BCY0/CCY0/DCY0 mux bits, but this is future-proofing, not a fix.

Full CARRY4 support requires: adder inference in helion-sv, DI/S/CYINIT capture in pack.rs/place.rs, and PRECYINIT emission in emit.rs. Out of scope for the blinky.

### 3. FF clock-enable / set-reset: FIXED (needs hardware verification)

`emit.rs` emitted ZINI and FFMUX but never ZRST, CEUSEDMUX, or SRUSEDMUX. Flip-flops silently ignored CE and SR pins. Fixed: ZRST emitted per-FF when sr_net present; CEUSEDMUX/SRUSEDMUX emitted per-slice (deduplicated). Committed, unit tests pass. Hardware verification pending.

See: `code/emit.rs` FF emit loop

### 4. Investigator audit: 22 anomalies, 8 critical

Full line-by-line audit in `findings/investigator_report.md`. Critical findings beyond the above:

- C5: `bufg_site()` always returns the first site, ignoring the `used` set. Two clocks collide on the same BUFG. Same bug in `ramb_site()`, `dsp_site()`.
- C6: CARRY4s placed on random slices; carry chains require vertical adjacency.
- C7: BRAM INIT parse failure silently becomes 0 (`unwrap_or(0)`).
- C8: BRAM emits only IN_USE, no port/clock/width config. Non-functional.
- C4: DSPs explicitly discarded (`let _ = &placed.dsps;`) with no error.

### 5. Test harness: 5 isolated tests ready

In `tests/`: t01 (IOB static, no clock), t02 (BUFG toggle, no arithmetic - THE key clock test), t03 (carry chain counter), t04 (FF clock-enable), t05 (FF reset). Each has expected LED behavior in `findings/test_manifest.md`. Bitstreams pending backend builds.

### 6. Design corpus: 21 designs

`findings/corpus_manifest.md` lists 21 open-source designs (12 inline reference, 5 verified GitHub, 4 candidates) for broader validation after the backend is fixed.

## What to try next

1. **BUFG polarity**: The prime suspect. Try inverting the ZINV_ bits, or find the definitive polarity in Xilinx UG472 / X-Ray docs. The t02_bufg_toggle test isolates this: single FF, no arithmetic. If LD0 glows at 50MHz, the clock works.
2. **Clock routing**: The BUFG output must reach the global clock network. Check whether route.rs emits the correct PIPs from BUFG_O to the clock spine.
3. **IOB sanity**: Run t01 first (no clock). If static LEDs don't light, the IOB output config is also wrong.
4. **Fail loudly**: Per the audit, make emit.rs return Err for DSP, BRAM, and multi-BUFG instead of generating silent broken bitstreams.

## Files in this package

- `README.md`: this file
- `code/`: snapshot of crates/helion-xilinx7/src (emit.rs, pack.rs, place.rs, route.rs, etc.)
- `tests/`: t01-t05 isolated tests, zedboard_blinky.sv, zedboard.xdc
- `xray-db/`: segbits_clk_bufg_bot_r.db, segbits_clbll_l.db
- `findings/`: investigator_report.md (22 anomalies), critique.md (critic verdicts), test_manifest.md (results log), corpus_manifest.md (21 designs)
- `docs/`: (pin map and programming notes, if present)

## Reproducing the build

```bash
# In the helion repo, worktree/branch with the fixes:
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --release -p helion-xilinx7 --test zedboard_e2e
# Bitstream lands at /tmp/x7/zedboard_blinky.bit
```

## Programming

```bash
openFPGALoader -b zedboard --write-sram <file.bit>
# Expect: "Load SRAM 100%", "done 1"
# Then observe LD0. Helion bitstreams: dark (broken). Vivado blinky.bit: blinks (reference).
```
