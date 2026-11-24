# zynq: board check of the PS7 rung (ZedBoard)

**Passes on the board: 15/15 (2026-11-24).** ZedBoard in JTAG boot mode, openocd
0.12.0 on macOS, bitstream from `zedbit` at 5c95cd42 (md5
60025d1f952a8c4099cd14668e6309ea, +4.254 ns). See "Board results" below.

It checks `examples/zedboard_ps7_axi.sv` on silicon: the ARM reads and writes
the PL registers over M_AXI_GP0, and FCLK0 is measured with the PL's cycle
counter.

## Run (ZedBoard, JTAG boot mode)

1. Set the boot-mode jumpers to JTAG (MIO[5:3] = 000, UG585 "Boot Mode Pin
   Settings" Table 1). Power on with the USB-JTAG (PROG) port connected.
2. Load the bitstream into the PL: `openFPGALoader -b zedboard out.bit`.
3. `openocd -f board/digilent_zedboard.cfg -f tools/zynq/ps7_axi_check.tcl -c "ps7_axi_check; shutdown"`

The script does the following:

1. Halts CPU0, unlocks the SLCR, and prints BOOT_MODE and the PLL state it found.
2. Moves the IO PLL to M = 30 (1000 MHz). It bypasses the PLL and puts it in reset, sets the loop settings and FDIV, waits for lock, and then removes the bypass. It sets FPGA0_CLK_CTRL to IO PLL / 10 / 1.
3. Checks that FCLK0 is not throttled and that devcfg reports the PL configured (DONE). It stops at this point if the PL is not configured, because GP0 would hang.
4. Enables the PS-PL level shifters, pulses and releases FCLKRESETN[3:0], and locks the SLCR.
5. Reads and writes every register through GP0, printing PASS/FAIL for each:
   - MAGIC;
   - SCRATCH and LEDS after reset;
   - SCRATCH with word, byte and halfword writes (WSTRB 1111, 0010, 1100, 0001);
   - LEDS bits 6:0, and that a byte-1 write leaves it alone.
6. Measures FCLK0 from COUNTER over a 1 s and a 21 s window and divides the difference by 20 s, so the JTAG read latency (35 to 300 ms per read on the board) cancels. It must be within 2% of 100 MHz.
7. Ends with LD0, LD2, LD4 and LD6 on. LD7 blinks at about 0.75 Hz (COUNTER bit 26).

The script returns the number of failed checks. The CPU is left halted.

Every SLCR address, field and key in the script is cited in its comments from
UG585 v1.15. Two values come from other sources:

- **IO PLL loop settings for M = 30** (PLL_CP 2, PLL_RES 12, LOCK_CNT 325). These are in UG585 "PLLs" Table 1 (Table 25-6 in the PDF), but that table's body is missing from the HTML edition. The values were taken from two transcriptions that agree: the zynq7000-hal crate, and Xilinx's U-Boot `zed_hw_platform/ps7_init_gpl.c`.
- **The devcfg PCFG_DONE bit position** (bit 2). It comes from Xilinx's `xdevcfg_hw.h`.

## Check without a board

```sh
tclsh tools/zynq/ps7_axi_check_mock.tcl
openocd -c "adapter driver dummy" -f target/zynq_7000.cfg \
  -f tools/zynq/ps7_axi_check.tcl -f tools/zynq/ps7_axi_check_mock.tcl -c shutdown
```

The mock replaces openocd's memory and target commands with a model. It
covers the SLCR registers the script touches, devcfg INT_STS, and the register
block. The model enforces three rules:

- SLCR writes are ignored while the SLCR is locked.
- FDIV may only change while the PLL is bypassed and in reset, and the PLL locks only with the M = 30 loop settings.
- No GP0 access is allowed before DONE, the level shifters and the reset release.

It runs six cases, and each must end as expected:

- PLL strap enabled;
- PLL strap bypassed;
- a second run with stale register values;
- a SCRATCH that ignores WSTRB, which must give 3 FAILs;
- PS_CLK of 50 MHz, where the FCLK0 check must FAIL;
- PL not configured, where the script must stop before GP0.

## Board results (2026-11-24)

First run on a ZedBoard, JTAG boot mode (BOOT_MODE read as 0x00000000), PL
loaded with `openFPGALoader -b zedboard`. Everything the list below used to call
unverified is now confirmed on silicon:

- openocd `halt`, `read_memory` and `write_memory` through cortex_a work. The CPU sits in the BootROM JTAG loop (pc 0xffffff34) with MMU and caches off.
- Each CPU store reaches GP0 as one write of its own size: the WSTRB 1111, 0010, 1100 and 0001 checks on SCRATCH and the byte-1 check on LEDS all pass.
- The IO PLL locks at M = 30 with PLL_CP 2, PLL_RES 12 and LOCK_CNT 325 (IO_PLL_CTRL 0x0001E000, PLL_STATUS 0x3F).
- FCLK0 is 100.02 MHz (this script) and 100.44 MHz (a separate 30 s differenced read), so PS_CLK is 33.333 MHz.
- devcfg INT_STS reads 0xF802000F after an openFPGALoader JTAG load: PCFG_DONE_INT is set.
- The design: MAGIC reads 0x484C0701, and the LEDs show 0x55 with the LD7 heartbeat blinking (seen on camera).

The first run scored 14/15. Its single 1 s FCLK0 window read 76.15 MHz, because the
JTAG reads around it took about 700 ms. The two-window measurement above replaced
it, the mock still passes 6/6 (the 50 MHz PS_CLK case still fails as it must),
and the second board run scored 15/15.
