# ZedBoard Hardware Test Corpus Manifest

**Purpose:** Open-source Verilog designs to validate the Helion 7-series backend on the ZedBoard (xc7z020). Every design must build through Helion, program via openFPGALoader, and produce observable correct behavior.

**Board constants:** 100 MHz GCLK on pin Y9. LEDs LD0-LD7 on pins T22, T21, U22, U21, V22, W22, U19, U14 (Bank 33, LVCMOS33, active high). Buttons BTNC/BTND/BTNL/BTNR/BTNU on P16, R16, N15, R18, T18.

**Primitive stress key:** BUFG = clock buffer config, CARRY4 = arithmetic carry chains, CE = FF clock-enable, SR = FF set/reset, IOB = pin config.

---

## Tier 1: Inline reference designs (no download needed)

These are small enough to embed directly. Each stresses specific primitives.

### 1.1 binary_counter_blinky
- **What:** 27-bit free-running counter, LED[i] = cnt[26-i]. LD0 blinks at 0.745 Hz.
- **Expected:** LD0 slow blink, LD1-LD5 progressively faster, LD6/LD7 may stay dark (X-Ray DB gap on RIOB33_SING).
- **Stresses:** BUFG (clock), CARRY4 (27-bit increment), IOB outputs.
- **Status:** This is the current bring-up design (`examples/zedboard_blinky.sv`).

### 1.2 shift_chaser
- **What:** 8-bit shift register, single 1 circulating. No arithmetic.
```verilog
module shift_chaser(input clk, output [7:0] led);
  reg [7:0] sr = 8'b00000001;
  always @(posedge clk) sr <= {sr[6:0], sr[7]};
  assign led = sr;
endmodule
```
- **Expected:** Single LED chasing across LD0-LD7 at visible rate (needs clock divider; use bit 24 of a counter as the shift clock).
- **Stresses:** BUFG, FF (no CE/SR), IOB. Isolates clock from CARRY4.

### 1.3 pwm_fader
- **What:** 8-bit free counter compared against a slowly ramping duty value; LED brightness breathes.
```verilog
module pwm_fader(input clk, output led);
  reg [7:0] cnt = 0;
  reg [7:0] duty = 0;
  reg [23:0] slow = 0;
  always @(posedge clk) begin
    cnt <= cnt + 1'b1;
    slow <= slow + 1'b1;
    if (slow == 0) duty <= duty + 1'b1;
  end
  assign led = (cnt < duty);
endmodule
```
- **Expected:** LED fades in/out smoothly.
- **Stresses:** BUFG, CARRY4 (two counters + comparator), IOB.

### 1.4 debouncer
- **What:** Button debouncer using a 17-bit counter; BTNC pressed -> LD0 on (debounced).
```verilog
module debouncer(input clk, input btn, output led);
  reg [16:0] cnt = 0;
  reg sync0 = 0, sync1 = 0;
  always @(posedge clk) begin
    sync0 <= btn; sync1 <= sync0;
    if (sync0 != sync1) cnt <= 0;
    else if (cnt != 17'h1FFFF) cnt <= cnt + 1'b1;
  end
  assign led = (cnt == 17'h1FFFF) ? sync1 : 1'b0;
endmodule
```
- **Expected:** LD0 follows BTNC with no flicker.
- **Stresses:** BUFG, CARRY4 (counter + comparator), FF CE (counter enable), IOB input + output.

### 1.5 traffic_fsm
- **What:** Traffic-light FSM: 3 states (GREEN/YELLOW/RED) with timed transitions driving LD0/LD1/LD2.
- **Expected:** LEDs cycle green -> yellow -> red with realistic timing.
- **Stresses:** BUFG, FSM (no arithmetic except timer counter), IOB.

### 1.6 lfsr_random
- **What:** 16-bit Fibonacci LFSR, LEDs show low 8 bits.
```verilog
module lfsr_random(input clk, output [7:0] led);
  reg [15:0] lfsr = 16'hACE1;
  always @(posedge clk)
    lfsr <= {lfsr[14:0], lfsr[15] ^ lfsr[13] ^ lfsr[12] ^ lfsr[10]};
  assign led = lfsr[7:0];
endmodule
```
- **Expected:** LEDs show pseudo-random pattern changing fast (use slow bit for visible rate).
- **Stresses:** BUFG, FF (no CARRY4, no CE/SR). Pure sequential test.

### 1.7 stopwatch
- **What:** 0.01s tick counter, counts up on LD[3:0] as BCD-ish nibble, BTNC resets.
- **Expected:** LEDs count visibly; BTNC resets to zero.
- **Stresses:** BUFG, CARRY4, FF SR (reset), IOB input + output.

### 1.8 servo_pwm
- **What:** 12-bit counter generating 1-2 ms servo pulse; duty set by BTNU/BTND.
- **Expected:** LED brightness/duty changes with buttons.
- **Stresses:** BUFG, CARRY4, comparator, FF CE.

### 1.9 johnson_counter
- **What:** 8-bit Johnson (twisted ring) counter; 2N states for N FFs.
```verilog
module johnson_counter(input clk, output [7:0] led);
  reg [7:0] j = 8'b00000000;
  always @(posedge clk) j <= {~j[0], j[7:1]};
  assign led = j;
endmodule
```
- **Expected:** Walking-ones then walking-zeros pattern on LEDs.
- **Stresses:** BUFG, FF (no CARRY4). Tests inversion in shift path.

### 1.10 btn_counter
- **What:** BTNU increments, BTND decrements an 8-bit counter shown on LEDs; BTNC resets.
- **Expected:** LEDs count up/down with buttons; BTNC clears.
- **Stresses:** BUFG, CARRY4 (up/down counter), FF SR, IOB inputs, button sync.

### 1.11 clkdiv_chain
- **What:** Chain of 27 toggle FFs (ripple divider); LED[i] = stage 19+i output.
- **Expected:** Each LED blinks at half the rate of the previous.
- **Stresses:** BUFG, FF only (no CARRY4, tests ripple clocking through FF Q pins).

### 1.12 pattern_gen
- **What:** 8-bit counter feeding a case statement mapping to LED patterns (chase, blink-all, alternate).
```verilog
module pattern_gen(input clk, output [7:0] led);
  reg [25:0] cnt = 0;
  reg [1:0] mode = 0;
  always @(posedge clk) begin
    cnt <= cnt + 1'b1;
    if (cnt == 0) mode <= mode + 1'b1;
  end
  assign led = (mode == 0) ? 8'h01 << cnt[25:23] :
               (mode == 1) ? (cnt[25] ? 8'hFF : 8'h00) :
               (mode == 2) ? 8'hAA : 8'h55;
endmodule
```
- **Expected:** LED patterns cycle every few seconds.
- **Stresses:** BUFG, CARRY4, wide muxes (LUT), IOB.

---

## Tier 2: Verified GitHub repositories

### 2.1 PicoRV32 (YosysHQ/picorv32)
- **URL:** https://github.com/YosysHQ/picorv32
- **File:** `picorv32.v` (single file, RV32IMC CPU)
- **What:** Size-optimized RISC-V CPU, 750-2000 LUTs on 7-series.
- **Test plan:** Wrap with a small program that writes a counter to LEDs via memory-mapped GPIO. Expected: LEDs count in a software-driven pattern (distinguishable from hardware counter).
- **Stresses:** Everything: BUFG, CARRY4 (ALU), FF CE/SR, BRAM (register file may infer), deep state machines.
- **Note:** Needs a firmware hex and memory wrapper; medium-effort bring-up. Defer until Tier 1 passes.

### 2.2 SERV (olofk/serv)
- **URL:** https://github.com/olofk/serv
- **Files:** `rtl/` directory (bit-serial RV32I CPU)
- **What:** World's smallest RISC-V CPU, 125 LUTs / 164 FFs on Artix-7.
- **Test plan:** Same GPIO-counter firmware approach as PicoRV32.
- **Stresses:** BUFG, FF-heavy (shift-register register file), minimal CARRY4.
- **Note:** Bit-serial ALU is unusual; good diversity vs PicoRV32. Defer until Tier 1 passes.

### 2.3 verilog-uart (alexforencich)
- **URL:** https://github.com/alexforencich/verilog-uart (canonical; seen via deepwiki page for this repo)
- **File:** `rtl/uart_tx.v` (AXI4-Stream UART transmitter)
- **What:** Production UART TX with baud-rate generator.
- **Test plan:** Tie AXI-Stream to a fixed byte, loop transmission; probe TX pin or map `busy` to LED. Expected: LED blinks at byte rate.
- **Stresses:** BUFG, CARRY4 (baud counter), FSM, FF CE/SR.
- **Verification status:** URL not yet fetched; verify before use.

### 2.4 FPGA-peripherals UART (FPGAwars)
- **URL:** https://github.com/FPGAwars/FPGA-peripherals (seen via project wiki page)
- **File:** UART TX peripheral
- **What:** Simple async serial transmitter, multiple baud rates.
- **Test plan:** Same as 2.3.
- **Verification status:** URL not yet fetched; verify before use.

### 2.5 ZipCPU dspfilters
- **URL:** https://github.com/ZipCPU/dspfilters (seen via zipcpu.com FIR article)
- **Files:** `rtl/firtap.v`, `rtl/genericfir.v`
- **What:** Parameterized FIR filter, uses MAC operations.
- **Test plan:** Feed a step input, observe filtered output on LEDs (threshold the output MSB).
- **Stresses:** CARRY4 (MAC adders), DSP48E1 if inferred (currently unsupported; expect fail-loud), FF CE.
- **Note:** DSP inference is out of scope; use small tap counts to stay in LUTs.
- **Verification status:** URL not yet fetched; verify before use.

---

## Tier 3: Candidates to evaluate

These were found in search; each needs URL verification and a quick RTL review before adding to the test rotation.

- **Project F FPGA tutorials** (projectf.io): binary counter, PWM, debouncer examples. Well-structured, educational. URL to verify.
- **chipmunklogic debouncer**: Synthesizable debouncer with Basys-3 test evidence. URL to verify.
- **TinyTapeout FIR** (tinytapeout.com): 4-coefficient FIR, small. Good DSP-adjacent test without DSP48.
- **WalkNSqualk state machine** (walknsqualk.com): 3-state LED FSM with testbench. Simple FSM coverage.

---

## Test procedure per design

1. Adapt RTL to ZedBoard: `clk` <- Y9, `led` -> T22/T21/U22/U21/V22/W22/U19/U14, `btn` <- P16/R16/N15/R18/T18 as needed.
2. Build bitstream with Helion 7-series backend.
3. Program: `openFPGALoader -b zedboard --write-sram <file>.bit`.
4. Confirm `done 1`.
5. Observe LEDs against the Expected behavior above.
6. Log PASS/FAIL with the observed vs expected behavior.

## Priority order

1. Tier 1.2 (shift_chaser): isolates BUFG from CARRY4. If this blinks, the clock works.
2. Tier 1.1 (binary_counter_blinky): the current failing design. Needs BUFG + CARRY4.
3. Tier 1.4 (debouncer): adds FF CE and IOB input.
4. Tier 1.7 (stopwatch): adds FF SR.
5. Tier 1.3/1.8 (PWM): comparator coverage.
6. Tier 2: CPU and peripheral designs after Tier 1 is green.
