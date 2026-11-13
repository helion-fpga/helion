# Digilent Arty A7-35T (xc7a35tcsg324-1): pins for Helion's 7-series flow.
# Source: Digilent Arty-A7-35-Master.xdc; every pin checked against the Project X-Ray
# package pin table for xc7a35tcsg324-1 (crates/helion-xilinx7/tests/boards.rs).
# Bank 34 is the DDR3 bank (1.35 V) and is not used. RGB LED order: led0_r, led0_g, led0_b, led1_r, ...

# Clock
set_property -dict { PACKAGE_PIN E3 IOSTANDARD LVCMOS33 } [get_ports { clk }]
create_clock -period 10.000 -name sys_clk [get_ports { clk }]

# Slide switches
set_property -dict { PACKAGE_PIN A8 IOSTANDARD LVCMOS33 } [get_ports { sw[0] }]
set_property -dict { PACKAGE_PIN C11 IOSTANDARD LVCMOS33 } [get_ports { sw[1] }]
set_property -dict { PACKAGE_PIN C10 IOSTANDARD LVCMOS33 } [get_ports { sw[2] }]
set_property -dict { PACKAGE_PIN A10 IOSTANDARD LVCMOS33 } [get_ports { sw[3] }]

# Push buttons (btn0, btn1, btn2, btn3)
set_property -dict { PACKAGE_PIN D9 IOSTANDARD LVCMOS33 } [get_ports { btn[0] }]
set_property -dict { PACKAGE_PIN C9 IOSTANDARD LVCMOS33 } [get_ports { btn[1] }]
set_property -dict { PACKAGE_PIN B9 IOSTANDARD LVCMOS33 } [get_ports { btn[2] }]
set_property -dict { PACKAGE_PIN B8 IOSTANDARD LVCMOS33 } [get_ports { btn[3] }]

# LEDs
set_property -dict { PACKAGE_PIN H5 IOSTANDARD LVCMOS33 } [get_ports { led[0] }]
set_property -dict { PACKAGE_PIN J5 IOSTANDARD LVCMOS33 } [get_ports { led[1] }]
set_property -dict { PACKAGE_PIN T9 IOSTANDARD LVCMOS33 } [get_ports { led[2] }]
set_property -dict { PACKAGE_PIN T10 IOSTANDARD LVCMOS33 } [get_ports { led[3] }]

# RGB LEDs (r, g, b per LED)
set_property -dict { PACKAGE_PIN G6 IOSTANDARD LVCMOS33 } [get_ports { rgb[0] }]
set_property -dict { PACKAGE_PIN F6 IOSTANDARD LVCMOS33 } [get_ports { rgb[1] }]
set_property -dict { PACKAGE_PIN E1 IOSTANDARD LVCMOS33 } [get_ports { rgb[2] }]
set_property -dict { PACKAGE_PIN G3 IOSTANDARD LVCMOS33 } [get_ports { rgb[3] }]
set_property -dict { PACKAGE_PIN J4 IOSTANDARD LVCMOS33 } [get_ports { rgb[4] }]
set_property -dict { PACKAGE_PIN G4 IOSTANDARD LVCMOS33 } [get_ports { rgb[5] }]
set_property -dict { PACKAGE_PIN J3 IOSTANDARD LVCMOS33 } [get_ports { rgb[6] }]
set_property -dict { PACKAGE_PIN J2 IOSTANDARD LVCMOS33 } [get_ports { rgb[7] }]
set_property -dict { PACKAGE_PIN H4 IOSTANDARD LVCMOS33 } [get_ports { rgb[8] }]
set_property -dict { PACKAGE_PIN K1 IOSTANDARD LVCMOS33 } [get_ports { rgb[9] }]
set_property -dict { PACKAGE_PIN H6 IOSTANDARD LVCMOS33 } [get_ports { rgb[10] }]
set_property -dict { PACKAGE_PIN K2 IOSTANDARD LVCMOS33 } [get_ports { rgb[11] }]

# Pmod JA
set_property -dict { PACKAGE_PIN G13 IOSTANDARD LVCMOS33 } [get_ports { ja[0] }]
set_property -dict { PACKAGE_PIN B11 IOSTANDARD LVCMOS33 } [get_ports { ja[1] }]
set_property -dict { PACKAGE_PIN A11 IOSTANDARD LVCMOS33 } [get_ports { ja[2] }]
set_property -dict { PACKAGE_PIN D12 IOSTANDARD LVCMOS33 } [get_ports { ja[3] }]
set_property -dict { PACKAGE_PIN D13 IOSTANDARD LVCMOS33 } [get_ports { ja[4] }]
set_property -dict { PACKAGE_PIN B18 IOSTANDARD LVCMOS33 } [get_ports { ja[5] }]
set_property -dict { PACKAGE_PIN A18 IOSTANDARD LVCMOS33 } [get_ports { ja[6] }]
set_property -dict { PACKAGE_PIN K16 IOSTANDARD LVCMOS33 } [get_ports { ja[7] }]

# Pmod JB
set_property -dict { PACKAGE_PIN E15 IOSTANDARD LVCMOS33 } [get_ports { jb[0] }]
set_property -dict { PACKAGE_PIN E16 IOSTANDARD LVCMOS33 } [get_ports { jb[1] }]
set_property -dict { PACKAGE_PIN D15 IOSTANDARD LVCMOS33 } [get_ports { jb[2] }]
set_property -dict { PACKAGE_PIN C15 IOSTANDARD LVCMOS33 } [get_ports { jb[3] }]
set_property -dict { PACKAGE_PIN J17 IOSTANDARD LVCMOS33 } [get_ports { jb[4] }]
set_property -dict { PACKAGE_PIN J18 IOSTANDARD LVCMOS33 } [get_ports { jb[5] }]
set_property -dict { PACKAGE_PIN K15 IOSTANDARD LVCMOS33 } [get_ports { jb[6] }]
set_property -dict { PACKAGE_PIN J15 IOSTANDARD LVCMOS33 } [get_ports { jb[7] }]
