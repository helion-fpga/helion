# Programming the ZedBoard with the Helion blinky bitstream

## What you need
- ZedBoard (xc7z020clg484-1) with 12V power supply
- Micro-USB cable
- MacBook with Homebrew

## Board setup
1. Connect the 12V power supply to the ZedBoard. Do NOT power on yet.
2. Connect a micro-USB cable from your MacBook to the ZedBoard's **J17**
   (the USB-JTAG port, labeled PROG on the silkscreen near the edge).
3. Flip the power switch **SW8** to ON. The red power LED lights up.

JTAG programming talks straight to the chip, so the boot-mode jumpers
(JP7-JP11) do not matter for this step. The bitstream only configures the
PL (FPGA fabric); the ARM cores are untouched.

## Install the programmer (one time)
openFPGALoader programs Xilinx 7-series parts over JTAG with no vendor
tools:

```
brew install openfpgaloader
```

## Program the bitstream
Copy `zedboard_blinky.bit` to your MacBook, then:

```
openFPGALoader -c digilent zedboard_blinky.bit
```

Expected output ends with `Done`. The DONE LED on the board lights up.

If the cable is not found, pass it explicitly by serial:
```
openFPGALoader --detect
```
then use the matching `-c digilent --ftdi-serial <serial>` if several
FTDI devices are attached.

## What success looks like
- **LD0-LD5** blink at decreasing rates: LD0 (pin T22) at about 0.75 Hz
  (roughly one toggle every 1.3 seconds), LD1-LD5 progressively faster
  (each taps a higher counter bit).
- **LD6 (pin U19) and LD7 (pin U14)** may stay dark: they sit on
  RIOB33_SING tiles, for which Project X-Ray's database ships no segment
  bits, so their IOB output-enable configuration could not be encoded.
  This is a known database gap, not a design bug. If LD0-LD5 blink, the
  PL is configured, the 100 MHz GCLK is running, and the pin map is
  correct.

## If it does not work
- DONE LED stays off: check the USB cable is in J17 (not the UART port),
  board power is on, and try `openFPGALoader --detect` to see the JTAG
  chain. Expected IDCODE: 0x03727093.
- LEDs do not blink but DONE is on: the bitstream loaded but the design
  is not running; report back with the openFPGALoader output.
- Fallback: Vivado Hardware Manager can program the same `.bit` file
  through the same USB-JTAG port.

## Rebuilding the bitstream
From the `fm/zedboard-bringup` branch of the Helion repo:

```
cargo test --release -p helion-xilinx7 --test zedboard_e2e
```

The bitstream lands at `/tmp/x7/zedboard_blinky.bit`. A verified copy
from the passing run is kept at:
`~/workspace/goals/zedboard-bring-up-xc7z020-db-pin-map-blinky-bitstream/files/zedboard_blinky.bit`
(3,159,295 bytes, IDCODE 0x03727093 at offset 235). Requires the X-Ray
zynq7 database (see `~/workspace/xilinx-dbsrc/zynq7/`).
