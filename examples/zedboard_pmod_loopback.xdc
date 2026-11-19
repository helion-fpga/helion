# ----------------------------------------------------------------------------
# ZedBoard (xc7z020clg484-1) pin constraints for
# examples/zedboard_pmod_loopback.sv.
#
# Primary source: official Digilent master XDC
#   https://github.com/Digilent/digilent-xdc/blob/master/Zedboard-Master.xdc
#   repo commit 00a3404901f35aa9567b01ecb3f2c233b6efe9f4, fetched 2026-11-19.
#   The file header notes its last change as 6 June 2016 (signal name
#   correction for package pin N19). Every PACKAGE_PIN below is copied
#   verbatim from that file; no pin was invented or guessed.
#
# Signal mapping for examples/zedboard_pmod_loopback.sv:
#   clk    <- GCLK, 100 MHz PL oscillator (Bank 13, fixed 3.3V)
#   ja[0]  <- JA1, ja[1] <- JA2, ja[2] <- JA3, ja[3] <- JA4,
#   ja[4]  <- JA7, ja[5] <- JA8, ja[6] <- JA9, ja[7] <- JA10 (Bank 13)
#   jb[0]  <- JB1, jb[1] <- JB2, jb[2] <- JB3, jb[3] <- JB4,
#   jb[4]  <- JB7, jb[5] <- JB8, jb[6] <- JB9, jb[7] <- JB10 (Bank 13)
#   led[0] <- LD0 ... led[7] <- LD7 (Bank 33, fixed 3.3V, active high)
#
# Hardware setup: eight jumper wires JA to JB, pin for pin
# (JA1-JB1, JA2-JB2, JA3-JB3, JA4-JB4, JA7-JB7, JA8-JB8, JA9-JB9, JA10-JB10).
# ----------------------------------------------------------------------------

# Clock: 100 MHz GCLK.
set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]

# JA Pmod - Bank 13 (driven outputs).
set_property PACKAGE_PIN Y11 [get_ports {ja[0]}]; # "JA1"
set_property PACKAGE_PIN AA11 [get_ports {ja[1]}]; # "JA2"
set_property PACKAGE_PIN Y10 [get_ports {ja[2]}]; # "JA3"
set_property PACKAGE_PIN AA9 [get_ports {ja[3]}]; # "JA4"
set_property PACKAGE_PIN AB11 [get_ports {ja[4]}]; # "JA7"
set_property PACKAGE_PIN AB10 [get_ports {ja[5]}]; # "JA8"
set_property PACKAGE_PIN AB9 [get_ports {ja[6]}]; # "JA9"
set_property PACKAGE_PIN AA8 [get_ports {ja[7]}]; # "JA10"

# JB Pmod - Bank 13 (sampled inputs).
set_property PACKAGE_PIN W12 [get_ports {jb[0]}]; # "JB1"
set_property PACKAGE_PIN W11 [get_ports {jb[1]}]; # "JB2"
set_property PACKAGE_PIN V10 [get_ports {jb[2]}]; # "JB3"
set_property PACKAGE_PIN W8 [get_ports {jb[3]}]; # "JB4"
set_property PACKAGE_PIN V12 [get_ports {jb[4]}]; # "JB7"
set_property PACKAGE_PIN W10 [get_ports {jb[5]}]; # "JB8"
set_property PACKAGE_PIN V9 [get_ports {jb[6]}]; # "JB9"
set_property PACKAGE_PIN V8 [get_ports {jb[7]}]; # "JB10"

# User LEDs LD0-LD7.
set_property PACKAGE_PIN T22 [get_ports {led[0]}]; # "LD0"
set_property PACKAGE_PIN T21 [get_ports {led[1]}]; # "LD1"
set_property PACKAGE_PIN U22 [get_ports {led[2]}]; # "LD2"
set_property PACKAGE_PIN U21 [get_ports {led[3]}]; # "LD3"
set_property PACKAGE_PIN V22 [get_ports {led[4]}]; # "LD4"
set_property PACKAGE_PIN W22 [get_ports {led[5]}]; # "LD5"
set_property PACKAGE_PIN U19 [get_ports {led[6]}]; # "LD6"
set_property PACKAGE_PIN U14 [get_ports {led[7]}]; # "LD7"

# Bank 13 is fixed to 3.3V on ZedBoard; Bank 33 is fixed to 3.3V as well.
set_property IOSTANDARD LVCMOS33 [get_ports -of_objects [get_iobanks 13]];
set_property IOSTANDARD LVCMOS33 [get_ports -of_objects [get_iobanks 33]];
