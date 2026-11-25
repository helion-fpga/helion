# ----------------------------------------------------------------------------
# ZedBoard (xc7z020clg484-1) pin constraints for examples/zedboard_vga.sv.
#
# Primary source: official Digilent master XDC
#   https://github.com/Digilent/digilent-xdc/blob/master/Zedboard-Master.xdc
#   repo commit 00a3404901f35aa9567b01ecb3f2c233b6efe9f4, fetched 2026-11-19.
#   The file header notes its last change as 6 June 2016 (signal name
#   correction for package pin N19). Every PACKAGE_PIN below is copied
#   verbatim from that file; no pin was invented or guessed.
#
# Signal mapping for examples/zedboard_vga.sv (VGA_R1 is the LSB of each
# 4-bit color channel, VGA_R4 the MSB; same for G and B):
#   clk       <- GCLK, 100 MHz PL oscillator (Bank 13, fixed 3.3V)
#   vga_r[0]  <- VGA-R1, vga_r[1] <- VGA-R2, vga_r[2] <- VGA-R3,
#   vga_r[3]  <- VGA-R4 (Bank 33)
#   vga_g[0]  <- VGA-G1, vga_g[1] <- VGA-G2, vga_g[2] <- VGA-G3,
#   vga_g[3]  <- VGA-G4 (Bank 33)
#   vga_b[0]  <- VGA-B1, vga_b[1] <- VGA-B2, vga_b[2] <- VGA-B3,
#   vga_b[3]  <- VGA-B4 (Bank 33)
#   vga_hs    <- VGA-HS (Bank 33)
#   vga_vs    <- VGA-VS (Bank 33)
#   led       <- LD0 (Bank 33, fixed 3.3V; toggles once per frame)
# ----------------------------------------------------------------------------

# Clock: 100 MHz GCLK.
set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]

# VGA Output - Bank 33.
set_property PACKAGE_PIN V20 [get_ports {vga_r[0]}]; # "VGA-R1"
set_property PACKAGE_PIN U20 [get_ports {vga_r[1]}]; # "VGA-R2"
set_property PACKAGE_PIN V19 [get_ports {vga_r[2]}]; # "VGA-R3"
set_property PACKAGE_PIN V18 [get_ports {vga_r[3]}]; # "VGA-R4"
set_property PACKAGE_PIN AB22 [get_ports {vga_g[0]}]; # "VGA-G1"
set_property PACKAGE_PIN AA22 [get_ports {vga_g[1]}]; # "VGA-G2"
set_property PACKAGE_PIN AB21 [get_ports {vga_g[2]}]; # "VGA-G3"
set_property PACKAGE_PIN AA21 [get_ports {vga_g[3]}]; # "VGA-G4"
set_property PACKAGE_PIN Y21 [get_ports {vga_b[0]}]; # "VGA-B1"
set_property PACKAGE_PIN Y20 [get_ports {vga_b[1]}]; # "VGA-B2"
set_property PACKAGE_PIN AB20 [get_ports {vga_b[2]}]; # "VGA-B3"
set_property PACKAGE_PIN AB19 [get_ports {vga_b[3]}]; # "VGA-B4"
set_property PACKAGE_PIN AA19 [get_ports {vga_hs}]; # "VGA-HS"
set_property PACKAGE_PIN Y19 [get_ports {vga_vs}]; # "VGA-VS"

# Frame heartbeat LED.
set_property PACKAGE_PIN T22 [get_ports {led}]; # "LD0"

# Bank 13 is fixed to 3.3V on ZedBoard; Bank 33 is fixed to 3.3V as well.
set_property IOSTANDARD LVCMOS33 [get_ports -of_objects [get_iobanks 13]];
set_property IOSTANDARD LVCMOS33 [get_ports -of_objects [get_iobanks 33]];
