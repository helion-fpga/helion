# zynq: board check of the PS7 rung (ZedBoard)

**Untested on hardware.** `ps7_axi_check.tcl` was written with no board
connected. It has only been run against `ps7_axi_check_mock.tcl`, under tclsh 9
and under openocd 0.12 with the dummy adapter. The first board session is its
first real run.

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
6. Measures FCLK0 from two COUNTER reads one second apart. It must be within 2% of 100 MHz.
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

## Unverified until the board session

- The script under openocd against the real chip: `halt`, and `read_memory`/`write_memory` through cortex_a.
- The assumption that each CPU store becomes a single GP0 write of its own size. With the MMU off (UG585 "APU and OCM State after BootROM"), accesses should be strongly ordered.
- The IO PLL actually locking at M = 30, and the loop settings above.
- FCLK0 at 100 MHz. This assumes the ZedBoard's PS_CLK is 33.333 MHz; the COUNTER check measures it.
- Whether devcfg's PCFG_DONE_INT is set when the PL is configured over JTAG by openFPGALoader. If not, the script stops at that step: check the printed INT_STS value.
- The design itself on silicon: GP0 transactions, byte strobes, the LEDs and the LD7 heartbeat.
