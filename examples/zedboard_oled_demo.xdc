# ----------------------------------------------------------------------------
# ZedBoard (xc7z020clg484-1) pin constraints for examples/zedboard_oled_demo.sv.
#
# Primary source: official Digilent master XDC
#   https://github.com/Digilent/digilent-xdc/blob/master/Zedboard-Master.xdc
#   repo commit 00a3404901f35aa9567b01ecb3f2c233b6efe9f4, fetched 2026-11-19.
#   The file header notes its last change as 6 June 2016 (signal name
#   correction for package pin N19). Every PACKAGE_PIN below is copied
#   verbatim from that file; no pin was invented or guessed.
#
# Signal mapping for examples/zedboard_oled_demo.sv:
#   clk       <- GCLK, 100 MHz PL oscillator (Bank 13, fixed 3.3V)
#   oled_dc   <- OLED-DC   (Bank 13)
#   oled_res  <- OLED-RES  (Bank 13)
#   oled_sclk <- OLED-SCLK (Bank 13)
#   oled_sdin <- OLED-SDIN (Bank 13)
#   oled_vbat <- OLED-VBAT (Bank 13)
#   oled_vdd  <- OLED-VDD  (Bank 13)
#   led       <- LD0 (Bank 33, fixed 3.3V, active high; lights when the
#                init sequence and display fill are done)
# ----------------------------------------------------------------------------

# Clock: 100 MHz GCLK.
set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]

# OLED Display - Bank 13.
set_property PACKAGE_PIN U10 [get_ports {oled_dc}]; # "OLED-DC"
set_property PACKAGE_PIN U9 [get_ports {oled_res}]; # "OLED-RES"
set_property PACKAGE_PIN AB12 [get_ports {oled_sclk}]; # "OLED-SCLK"
set_property PACKAGE_PIN AA12 [get_ports {oled_sdin}]; # "OLED-SDIN"
set_property PACKAGE_PIN U11 [get_ports {oled_vbat}]; # "OLED-VBAT"
set_property PACKAGE_PIN U12 [get_ports {oled_vdd}]; # "OLED-VDD"

# Done LED.
set_property PACKAGE_PIN T22 [get_ports {led}]; # "LD0"

# Bank 13 is fixed to 3.3V on ZedBoard; Bank 33 is fixed to 3.3V as well.
set_property IOSTANDARD LVCMOS33 [get_ports -of_objects [get_iobanks 13]];
set_property IOSTANDARD LVCMOS33 [get_ports -of_objects [get_iobanks 33]];
