# Rung 5 review (fm/zedboard-rung5 at 8b9f2d9)

Checked on the Mac on 2026-11-19 with the fm/zedboard-merge
binaries (zedbit and hnf), the prjxray database and verilator. Nothing on
fm/zedboard-rung5 was changed; fixes stay with the owner of those files.

## Results

| Design | svcheck (RTL vs Helion netlist, 3,000 cycles, --regs) | zedbit (100 MHz) | x7v verify (--no-reset, verilator) |
| --- | --- | --- | --- |
| zedboard_vga | PASS, 45,000 samples equal | 163 LUTs, 25 FF, 30 carry, slack +6.787 ns, 0 of 45 failing | PASS: 0 unexplained bits, 1,700,000 cycles (more than one 1,680,000-cycle frame, so VS is covered), 51,000,000 output samples, 0 mismatches, 0 X/Z |
| zedboard_pmod_loopback | PASS, 48,000 samples equal | 67 LUTs, 36 FF, 5 carry, slack +6.125 ns, 0 of 68 failing | PASS: 0 unexplained bits, 500,000 cycles, 16,000,000 output samples, 0 mismatches, 0 X/Z |
| zedboard_oled | FAIL: synthesis error, line 136 | not built | not run |

Every PACKAGE_PIN in the three .xdc files matches the Digilent ZedBoard master
XDC.

## zedboard_oled: four problems, all must be fixed before the board

1. **Line 136 does not parse.** `rom_entry(rom_idx)[7:0]` is a part-select of a
   function call. IEEE 1800 does not allow a select on a function call result,
   and Helion rejects it ("range of something that is not a signal"). Fix:
   `logic [15:0] entry; assign entry = rom_entry(rom_idx);` and use
   `entry[7:0]` and `entry[15:8]`.

2. **Command arguments are sent as display data.** The ROM marks argument
   bytes (0x80 after 0xD5, 0x1F after 0xA8, 0x14 after 0x8D, and so on) with
   bit 8 set, and line 137 drives D/C high for them. On the SSD1306, D/C high
   means "write to display RAM"; every byte of a command, its arguments
   included, must go with D/C low (SSD1306 datasheet, section 8.1 "MCU
   interface", D/C# pin, and the command table in section 9). With D/C high the
   arguments land in GDDRAM and each multi-byte command eats the next command
   byte as its argument, so the init sequence is garbled. Fix: D/C low for all
   25 init bytes; D/C high only for the 512 display bytes. The is_data flag in
   the ROM is then not needed.

3. **SDIN changes on the same clock edge that raises SCLK.** Lines 160 to 163
   set `oled_sdin` and `oled_sclk <= 1` in the same cycle, so the data and the
   rising edge leave the pads together with no setup time (the SSD1306 needs
   tens of ns of data setup before the SCLK rising edge). The comment says
   "falling edge setup", but the code does not do that. Fix: put the next bit
   on SDIN when SCLK goes low (and the first bit when the byte is loaded), and
   only raise SCLK half a period later.

4. **VDD and VBAT polarity needs a source.** The design drives `oled_vdd` and
   `oled_vbat` high to switch them on. On the PmodOLED, which uses the same
   driver circuit, VDDC and VBATC are active low. Please find the ZedBoard
   schematic page or the ZedBoard Hardware User's Guide OLED section and cite
   the polarity. If they are active low, the design as written keeps the panel
   off.

Smaller points (fix if cheap):
- SSD1306 power-up order is VDD, then RES low then high, then VBAT, then wait
  about 100 ms before 0xAF display on. The design pulses RES after VBAT, and
  waits 1 ms after VBAT.
- A testbench with a small SSD1306 SPI model (sample SDIN on SCLK rising,
  check D/C per byte, check the 25 init bytes and 512 data bytes) would have
  caught 2 and 3. It needs a machine with a simulator; the Mac here has
  one.

## zedboard_vga

Builds and checks clean. One low-severity note: the colour outputs come
straight from combinational logic on the counters, so they can glitch at each
counter change. Registering vga_r/g/b, vga_hs and vga_vs (one clock later on
all of them, so they stay aligned) is the usual practice.

## zedboard_pmod_loopback

Builds and checks clean. One coverage note: the "walking" patterns only use
bits 0 and 7, so swapped middle wires are only caught later by the
incrementing counter (one value every 8 ms, all 256 values in about 2 s). Fine
for a board check if the result is read after a few seconds.

## PS7 fact sheet (fm/zedboard-ps7-facts at a45d8bb)

Compared with tools/zynq/ps7_axi_check.tcl on fm/rung7-ps7. Every address, key
and field the script uses matches the sheet: SLCR_UNLOCK 0xF8000008 key
0xDF0D, SLCR_LOCK 0xF8000004 key 0x767B, SLCR_LOCKSTA 0xF800000C,
FPGA0_CLK_CTRL 0xF8000170 (DIVISOR1 25:20, DIVISOR0 13:8, SRCSEL 5:4),
IO_PLL_CTRL 0xF8000108, BOOT_MODE 0xF800025C bit 4, LVL_SHFTR_EN 0xF8000900
= 0xF, and FPGA_RST_CTRL 0xF8000240 written as whole words (0xF, then 0), not
read-modify-write, which avoids the reserved-bit trap the sheet points out.
The script reprograms the IO PLL to 30 x 33.333 MHz = 1,000 MHz and sets
DIVISOR0 = 10 for 100 MHz; that works whatever the PLL strap is, so the
sheet's 866.658 MHz BootROM state is read but not relied on.
