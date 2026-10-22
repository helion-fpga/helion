# ----------------------------------------------------------------------------
# ZedBoard (xc7z020clg484-1) pin constraints for the Helion 7-series backend.
#
# Primary source: official Digilent master XDC
#   https://github.com/Digilent/digilent-xdc/blob/master/Zedboard-Master.xdc
# (repo cloned 2026-10-22; every PACKAGE_PIN below is copied verbatim from
# that file; no pin was invented or guessed).
#
# Signal mapping for examples/zedboard_blinky.sv (top uses clk and led[7:0];
# btn[4:0] pins are provided here for bring-up use):
#   clk    <- GCLK, 100 MHz PL oscillator (Bank 13, fixed 3.3V)
#   led[0] <- LD0 ... led[7] <- LD7 (Bank 33, fixed 3.3V, active high)
#   btn[0] <- BTNC, btn[1] <- BTND, btn[2] <- BTNL,
#   btn[3] <- BTNR, btn[4] <- BTNU (Bank 34)
#
# Note: the master XDC defaults Bank 34 to LVCMOS18 because its Vcco follows
# the Vadj jumper (J18, factory default 1.8V). The LVCMOS33 constraint below
# for btn[4:0] is correct only when J18 is set to 3.3V; with J18 at 1.8V,
# switch the button IOSTANDARD to LVCMOS18.
# ----------------------------------------------------------------------------

# Clock: 100 MHz GCLK.
set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]

# User LEDs LD0-LD7.
set_property PACKAGE_PIN T22 [get_ports {led[0]}]
set_property PACKAGE_PIN T21 [get_ports {led[1]}]
set_property PACKAGE_PIN U22 [get_ports {led[2]}]
set_property PACKAGE_PIN U21 [get_ports {led[3]}]
set_property PACKAGE_PIN V22 [get_ports {led[4]}]
set_property PACKAGE_PIN W22 [get_ports {led[5]}]
set_property PACKAGE_PIN U19 [get_ports {led[6]}]
set_property PACKAGE_PIN U14 [get_ports {led[7]}]
set_property IOSTANDARD LVCMOS33 [get_ports {led}]

# User push buttons BTNC/BTND/BTNL/BTNR/BTNU.
set_property PACKAGE_PIN P16 [get_ports {btn[0]}]
set_property PACKAGE_PIN R16 [get_ports {btn[1]}]
set_property PACKAGE_PIN N15 [get_ports {btn[2]}]
set_property PACKAGE_PIN R18 [get_ports {btn[3]}]
set_property PACKAGE_PIN T18 [get_ports {btn[4]}]
set_property IOSTANDARD LVCMOS33 [get_ports {btn}]
