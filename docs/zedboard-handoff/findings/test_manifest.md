# ZedBoard Hardware Test Manifest

Isolated primitive tests for the Helion 7-series backend. Each test isolates
one primitive so a failure points at exactly one backend workstream.
Bitstreams are built by the bufg/carry4/ffctrl workers via the e2e harness
pattern; this manifest defines what to observe on the board.

Pin map (examples/zedboard.xdc): clk=Y9 (100 MHz), LD0=T22, LD1=T21, LD2=U22,
LD3=U21, LD4=V22, LD5=W22, LD6=U19, LD7=U14, BTNC=P16.
LD6/LD7 are on RIOB33_SING tiles with no X-Ray segment bits; they are
excluded from all pass criteria.

## Tests (run in order)

### t01_iob_static — IOB output driver
- File: tests/t01_iob_static.sv, bitstream: t01_iob_static.bit
- Isolates: OBUF / IOB output config. No clock, no FFs.
- EXPECT: LD0-LD4 brightly lit, LD5 dark.
- PASS: LD0,LD1,LD2,LD3,LD4 bright; LD5 dark.
- FAIL: any of LD0-LD4 dark, or LD5 lit.
- Backend workstream: IOB (verify).

### t02_bufg_toggle — BUFGCTRL clock path
- File: tests/t02_bufg_toggle.sv, bitstream: t02_bufg_toggle.bit
- Isolates: IBUF -> BUFGCTRL -> clock network -> FF. No CARRY4/CE/SR.
- EXPECT: LD0 glows DIM (50 MHz toggle, 50% duty). LD1-LD7 dark.
- PASS: LD0 visibly dim (on but not full bright).
- FAIL: LD0 fully dark (clock never reached the FF).
- Backend workstream: bufg.

### t03_carry4_counter — CARRY4 chain
- File: tests/t03_carry4_counter.sv, bitstream: t03_carry4_counter.bit
- Isolates: 26-bit incrementer needs 7 chained CARRY4s.
- EXPECT: LD0 blinks ~1.5 Hz, LD1-LD5 progressively faster.
- PASS: visible blink cascade on LD0-LD5.
- FAIL: LEDs stuck, wrong rates, or no activity.
- Backend workstream: carry4.

### t04_ff_ce — FF clock enable (needs BTNC)
- File: tests/t04_ff_ce.sv, bitstream: t04_ff_ce.bit
- Isolates: FF CEUSEDMUX. q_on (CE=1) toggles, q_off (CE=0) holds.
- EXPECT: LD0 dim, LD1 dark. LD2 follows BTNC (button sanity).
- PASS: LD0 dim AND LD1 dark AND LD2 responds to BTNC.
- FAIL: LD1 dim/glowing (CE ignored, q_off toggled).
- INCONCLUSIVE: LD2 does not respond to BTNC (Bank34 Vcco/J18 issue).
- Backend workstream: ffctrl.

### t05_ff_sr — FF synchronous reset (needs BTNC)
- File: tests/t05_ff_sr.sv, bitstream: t05_ff_sr.bit
- Isolates: FF SRUSEDMUX. BTNC forces q to 0 synchronously.
- EXPECT: LD0 dim normally; holding BTNC makes LD0 go dark; release resumes.
- PASS: LD0 dark while BTNC held, dim otherwise, LD2 mirrors button.
- FAIL: BTNC has no effect on LD0.
- INCONCLUSIVE: LD2 does not respond to BTNC.
- Backend workstream: ffctrl.

## Legacy bitstreams (already in bitstreams/)
- zedboard_blinky.bit: original Helion blinky (pre-fix). Programs OK, LEDs dark.
- zedboard_blinky_fixed.bit: partial BUFG fix. Programs OK, LEDs dark.
- zedboard_blinky_v2.bit: full BUFG (10 bits) + CARRY4 mux bits. Programs OK, LEDs dark.

## Results log
| Date (EDT) | Test | Program | Observed | Verdict | Notes |
|---|---|---|---|---|---|
| 2026-10-22 ~20:41 | zedboard_blinky.bit | done=1 | LEDs dark | FAIL | BUFG clock not passing |
| 2026-10-22 ~20:55 | zedboard_blinky_fixed.bit | done=1 | LEDs dark | FAIL | partial BUFG fix insufficient |
| 2026-10-22 ~21:15 | zedboard_blinky_v2.bit | done=1 | LEDs dark | FAIL | full BUFG + CARRY4 mux still insufficient |
| 2026-10-22 ~21:22 | zedboard_blinky.bit (round-robin) | done=1 | LEDs dark | FAIL | user confirmed no blink, original pre-fix bitstream |
| | t01_iob_static | | | | bitstream pending backend build |
| | t02_bufg_toggle | | | | bitstream pending backend build |
| | t03_carry4_counter | | | | bitstream pending backend build |
| | t04_ff_ce | | | | bitstream pending backend build |
| | t05_ff_sr | | | | bitstream pending backend build |
