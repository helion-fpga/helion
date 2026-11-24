# ZedBoard rung 5: HDMI out and audio out

Two PL-only designs, both clocked by the 100 MHz GCLK, no Zynq PS:

| Design | Files | What you should see or hear |
|---|---|---|
| `zedboard_hdmi` | `examples/zedboard_hdmi.sv`, `.xdc`, `_tb.sv` | 640x480 at 60 Hz, eight colour bars (white, yellow, cyan, green, magenta, red, blue, black) on a monitor on the HDMI jack. LD0 on once the ADV7511 is programmed, LD1 blinks about once a second. |
| `zedboard_audio` | `examples/zedboard_audio.sv`, `.xdc`, `_tb.sv` | A 999 Hz square wave at -18 dBFS on HPH OUT and LINE OUT, both channels. LD0 on once the ADAU1761 is programmed, LD1 blinks (0.36 s). |

## How they work

Each design has the same small write-only I2C master (a ROM of bytes, 50 kHz
SCL, 10 us low and 10 us high; while SCL is low a data bit changes 2.5 us
after the fall and the ACK level 50 ns after the 8th fall, inside the
standard-mode tVD;DAT / tVD;ACK maximum of 3.45 us; the testbenches check
these and the other NXP UM10204 Rev. 7.0 Table 11 limits listed in their
headers) and a free-running stream:

- HDMI: 16-bit YCbCr 4:2:2 (Y on HD-D[7:0], Cb/Cr alternating on HD-D[15:8],
  Cb first), separate HSYNC/VSYNC/DE, 25 MHz HD-CLK made in fabric (100 MHz
  / 4). Data changes when HD-CLK falls; 20 ns setup and hold at each rising
  edge. The ADV7511's CSC converts to RGB and it sends DVI. The bars are
  BT.709 limited-range YCbCr, because the CSC table (Linux
  `adv7511_csc_ycbcr_to_rgb`, the one ADI's driver loads) is a BT.709 matrix:
  its Cr->R / Y ratio is 1.541 (BT.709 1.540, BT.601 1.371). The first version
  sent BT.601 bars, which that table would show as e.g. red (255, 24, 0) and
  green (0, 217, 0). The decode model (coefficients x 4 / 4096, offsets
  x 4 / 16) is TODO-UNVERIFIED against the Programming Guide.
- Audio: the PL is I2S master. MCLK 12.5 MHz (100/8), BCLK 2.941 MHz
  (100/34), LRCLK = fs = 45955.9 Hz (100 MHz / 2176). The codec PLL makes
  its 1024 x fs core clock from MCLK with R = 3, N = 13, M = 17 (exact:
  12.5 MHz x 64/17), so the converters are locked to the PL's frame clock.

## Sources

Primary documents fetched for this work: the Digilent ZedBoard master XDC
(commit 00a3404), Analog Devices' ZedBoard HDL constraints and HDMI core
(analogdevicesinc/hdl `projects/common/zed`, `library/axi_hdmi_tx`,
`library/common/ad_ss_444to422.v`), Analog Devices' ZedBoard device tree
(analogdevicesinc/linux `zynq-zed-adv7511.dtsi`), the Linux drivers
`adv7511_drv.c`, `adv7511.h`, `adau17x1.c`, `adau17x1.h`, `adau1761.c`,
`adau-utils.c` (v6.6), the TU Kaiserslautern / Mike Field ZedBoard audio
design (github ems-kl/zedboard_audio), and NXP UM10204 Rev. 7.0.

The ADV7511 Programming Guide and the ADAU1761 datasheet could not be
fetched (analog.com refused connections; the Digilent reference site
returned a bot challenge, so the ZedBoard schematic and hardware guide were
not read either). Every register value is therefore taken from Analog
Devices' own driver and device tree for this board, and cross-checked
against a second design proven on the board where one exists. Each source
is cited next to its value in the `.sv` and `.xdc` headers.

| Item | Value | Source | Status |
|---|---|---|---|
| HDMI pins, I2C pins | master XDC HDMI section | Digilent XDC; same in ADI constraints | confirmed, 2 sources |
| Audio pins incl. ADR0/1 | master XDC Audio section | Digilent XDC; ADI constraints (all but ADR0/1) | confirmed |
| ADV7511 I2C address | 0x39 (0x72) | ADI zed device tree | confirmed (ADI) |
| ADV7511 power-up, fixed regs, 0x15/0x16/0x48/0xBA, CSC, 0xAF, 0xD6 | see `zedboard_hdmi.sv` | Linux adv7511 driver + ADI zed DT settings | TODO-UNVERIFIED against the Programming Guide |
| 16-bit bus layout | {C, Y}, Cb first | ADI `ad_ss_444to422.v` | confirmed (ADI) |
| ADAU1761 I2C address | 0x3B (0x76), ADR0 = ADR1 = 1 | ADI zed DT; ems-kl design | confirmed, 2 sources |
| ADAU1761 register map and bit fields | see `zedboard_audio.sv` | Linux adau17x1/adau1761 | TODO-UNVERIFIED against the datasheet |
| PLL byte encoding | `R << 3 \| X << 1 \| frac` | Linux `adau_calc_pll_cfg`; reproduces ems-kl's board-proven bytes | confirmed by cross-check |
| R0 INFREQ = 11 with PLL source | 0x0E / 0x0F | ems-kl (Linux leaves INFREQ = 00) | TODO-UNVERIFIED |
| Dejitter re-init after power-up (codec is I2S slave) | 0x4036 = 0x00 then 0x03, last two writes | Linux adau1761.c `adau1761_dejitter_fixup` (ems-kl skips it, codec master there) | TODO-UNVERIFIED against the datasheet (R67) |
| PLL lock time, PLL input/output range, R = 3 allowed | 20 ms wait; 12.5 MHz in, 47.06 MHz out | Linux limits (input <= 13.5 MHz, R 2..8) | TODO-UNVERIFIED |
| Codec at fs = 45.956 kHz | core 47.06 MHz | between the 44.1 and 48 kHz cores | TODO-UNVERIFIED |
| No clock stretching by either chip | push-pull SCL | none | TODO-UNVERIFIED |
| 25.000 MHz pixel clock accepted | 0.7 % under 25.175 MHz | VGA design uses the same | TODO-UNVERIFIED for ADV7511 |

## Limits

- Helion's v2 front end rejects `inout` on the top module and has no
  tristate output primitive, so SDA and SCL are push-pull. The master drives
  SDA low for each ACK slot (the level an acknowledging slave drives),
  starting 50 ns after SCL falls on the 8th bit. UM10204 lets a slave start
  its ACK anywhere from 0 ns to 3.45 us after that fall, so the FPGA can
  still drive a 1 against a slave for up to 50 ns per byte (the first
  version held the bit for 5 us: up to 5 us of contention on every byte
  ending in 1). A NACK cannot be detected: there is no ACK-error LED and no
  PLL-lock read-back.
- Board run (2026-11-24): see "On the board" below. Neither picture nor sound
  has been seen or heard yet.

## Checks

Run from the repository root (zedbit and hnf are release builds of the
`zedbit` and `hnf` examples):

```
verilator --binary --timing --top-module zedboard_hdmi_tb examples/zedboard_hdmi_tb.sv examples/zedboard_hdmi.sv
SVCHECK_HNF=hnf python3 tools/svcheck/svcheck.py examples/zedboard_hdmi.sv --top zedboard_hdmi --cycles 200000 --regs
zedbit examples/zedboard_hdmi.sv examples/zedboard_hdmi.xdc zedboard_hdmi.bit zedboard_hdmi
(cd tools/x7v && python3 -m x7v --board zedboard verify zedboard_hdmi.bit --rtl ../../examples/zedboard_hdmi.sv \
   --top zedboard_hdmi --xdc ../../examples/zedboard_hdmi.xdc --cycles 2500000 --no-reset)
```

and the same for `zedboard_audio` with `--cycles 3800000`.

Results (2026-11-23, Verilator 5.052, prebuilt zedbit/hnf from fm/zedboard-rung5):

| Check | zedboard_hdmi | zedboard_audio |
|---|---|---|
| Testbench | PASS: 41 I2C writes, 123 ACK hand-overs, 34 frames checked, last STOP at 23.672 ms (600 ms simulated) | PASS: 20 I2C writes, 85 ACK hand-overs, PLL wait 20.010 ms, 18390 frames, 799 tone half periods, last STOP at 36.096 ms (400 ms simulated) |
| Testbench catches faults | wrong ROM value (0x16=0x30), wrong bar luma, HSYNC one clock long, V_FRONT 11 or 30 (porch check), BT.601 green bar, ACK low only at the next quarter (old timing, 33 bytes) or 20 ns after SCL falls (hold check), SDA changed on the same edge as SCL, data bit 3.5 us or 5 us after SCL falls (tVD;DAT check): each fails | wrong PLL byte, BCLK divider 16, tone half period 22, ACK low only at the next quarter (old timing, 22 bytes), data bit 5 us after SCL falls (tVD;DAT check), dejitter writes missing or second one 0x00: each fails |
| svcheck, 200k cycles, --regs | PASS, 4,800,000 output samples equal | PASS, 2,000,000 output samples equal |
| zedbit at 100 MHz | 403 LUTs, 100 flops, 37 CARRY4, slack +3.242 ns | 251 LUTs, 97 flops, 13 CARRY4, slack +5.275 ns |
| x7v verify --rtl --no-reset | PASS, 2,500,000 cycles (whole init), 0 mismatches, 0 unexplained bits | PASS, 3,800,000 cycles (whole init), 0 mismatches, 0 unexplained bits |

## On the board (2026-11-24)

These bitstreams were built at f29a8590 with the prebuilt zedbit (timed, no
`--ignore-timing`) and loaded over JTAG with `openFPGALoader -b zedboard`.

| Design | Bitstream md5 | Slack | Seen on the board | Result |
|---|---|---|---|---|
| `zedboard_hdmi` | f8d121e8c0e866bb2cd8751a000e419e | +3.242 ns | LD0 on (ROM sent), LD1 blinks about once a second (frame counter runs) | PARTIAL: the only HDMI sink was a broken TV, so the bars were not seen |
| `zedboard_audio` | f030568988ce6a4711dbb5af06534ddf | +5.275 ns | LD0 on (ROM sent, after the 20 ms PLL wait), LD1 blinks fast (LRCLK-derived) | PARTIAL: nothing was plugged into HPH OUT or LINE OUT |

LD0 shows only that the I2C ROM was sent. With push-pull SDA and no ACK
read-back (see Limits), a wrong address or register value looks the same on the
LEDs. A pass needs the colour bars on a monitor (or a USB HDMI capture device) and
the 999 Hz tone on headphones.
