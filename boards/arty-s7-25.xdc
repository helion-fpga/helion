# Digilent Arty S7-25 (xc7s25csga324-1): pins for Helion's 7-series flow.
# Source: Digilent Arty-S7-25-Master.xdc; every pin checked against the Project X-Ray
# package pin table for xc7s25csga324-1 (crates/helion-xilinx7/tests/boards.rs).
# Bank 34 is the DDR3L bank (1.35 V, SSTL135) and is not used, so neither is the
# 100 MHz clock on R2 nor sw[3] on M5. RGB LED order: led0_r, led0_g, led0_b, led1_r, ...

# Clock: 12 MHz oscillator
set_property -dict { PACKAGE_PIN F14 IOSTANDARD LVCMOS33 } [get_ports { clk }]
create_clock -period 83.333 -name sys_clk [get_ports { clk }]

# Slide switches (sw[3] is on bank 34)
set_property -dict { PACKAGE_PIN H14 IOSTANDARD LVCMOS33 } [get_ports { sw[0] }]
set_property -dict { PACKAGE_PIN H18 IOSTANDARD LVCMOS33 } [get_ports { sw[1] }]
set_property -dict { PACKAGE_PIN G18 IOSTANDARD LVCMOS33 } [get_ports { sw[2] }]

# Push buttons (btn0, btn1, btn2, btn3)
set_property -dict { PACKAGE_PIN G15 IOSTANDARD LVCMOS33 } [get_ports { btn[0] }]
set_property -dict { PACKAGE_PIN K16 IOSTANDARD LVCMOS33 } [get_ports { btn[1] }]
set_property -dict { PACKAGE_PIN J16 IOSTANDARD LVCMOS33 } [get_ports { btn[2] }]
set_property -dict { PACKAGE_PIN H13 IOSTANDARD LVCMOS33 } [get_ports { btn[3] }]

# LEDs (LD2..LD5)
set_property -dict { PACKAGE_PIN E18 IOSTANDARD LVCMOS33 } [get_ports { led[0] }]
set_property -dict { PACKAGE_PIN F13 IOSTANDARD LVCMOS33 } [get_ports { led[1] }]
set_property -dict { PACKAGE_PIN E13 IOSTANDARD LVCMOS33 } [get_ports { led[2] }]
set_property -dict { PACKAGE_PIN H15 IOSTANDARD LVCMOS33 } [get_ports { led[3] }]

# RGB LEDs (r, g, b per LED)
set_property -dict { PACKAGE_PIN J15 IOSTANDARD LVCMOS33 } [get_ports { rgb[0] }]
set_property -dict { PACKAGE_PIN G17 IOSTANDARD LVCMOS33 } [get_ports { rgb[1] }]
set_property -dict { PACKAGE_PIN F15 IOSTANDARD LVCMOS33 } [get_ports { rgb[2] }]
set_property -dict { PACKAGE_PIN E15 IOSTANDARD LVCMOS33 } [get_ports { rgb[3] }]
set_property -dict { PACKAGE_PIN F18 IOSTANDARD LVCMOS33 } [get_ports { rgb[4] }]
set_property -dict { PACKAGE_PIN E14 IOSTANDARD LVCMOS33 } [get_ports { rgb[5] }]

# Pmod JA
set_property -dict { PACKAGE_PIN L17 IOSTANDARD LVCMOS33 } [get_ports { ja[0] }]
set_property -dict { PACKAGE_PIN L18 IOSTANDARD LVCMOS33 } [get_ports { ja[1] }]
set_property -dict { PACKAGE_PIN M14 IOSTANDARD LVCMOS33 } [get_ports { ja[2] }]
set_property -dict { PACKAGE_PIN N14 IOSTANDARD LVCMOS33 } [get_ports { ja[3] }]
set_property -dict { PACKAGE_PIN M16 IOSTANDARD LVCMOS33 } [get_ports { ja[4] }]
set_property -dict { PACKAGE_PIN M17 IOSTANDARD LVCMOS33 } [get_ports { ja[5] }]
set_property -dict { PACKAGE_PIN M18 IOSTANDARD LVCMOS33 } [get_ports { ja[6] }]
set_property -dict { PACKAGE_PIN N18 IOSTANDARD LVCMOS33 } [get_ports { ja[7] }]

# Pmod JB
set_property -dict { PACKAGE_PIN P17 IOSTANDARD LVCMOS33 } [get_ports { jb[0] }]
set_property -dict { PACKAGE_PIN P18 IOSTANDARD LVCMOS33 } [get_ports { jb[1] }]
set_property -dict { PACKAGE_PIN R18 IOSTANDARD LVCMOS33 } [get_ports { jb[2] }]
set_property -dict { PACKAGE_PIN T18 IOSTANDARD LVCMOS33 } [get_ports { jb[3] }]
set_property -dict { PACKAGE_PIN P14 IOSTANDARD LVCMOS33 } [get_ports { jb[4] }]
set_property -dict { PACKAGE_PIN P15 IOSTANDARD LVCMOS33 } [get_ports { jb[5] }]
set_property -dict { PACKAGE_PIN N15 IOSTANDARD LVCMOS33 } [get_ports { jb[6] }]
set_property -dict { PACKAGE_PIN P16 IOSTANDARD LVCMOS33 } [get_ports { jb[7] }]
