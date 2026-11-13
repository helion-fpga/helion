# Avnet/Digilent ZedBoard (xc7z020clg484-1): pins for Helion's 7-series flow.
# Source: Digilent ZedBoard master XDC (digilent-xdc, Zedboard-Master.xdc); every pin checked against the Project X-Ray
# package pin table for xc7z020clg484-1 (crates/helion-xilinx7/tests/boards.rs).
# Switches and buttons are on the Vadj banks (34, 35): LVCMOS18 matches the factory J18 setting (1.8 V).

# Clock
set_property -dict { PACKAGE_PIN Y9 IOSTANDARD LVCMOS33 } [get_ports { clk }]
create_clock -period 10.000 -name sys_clk [get_ports { clk }]

# Slide switches
set_property -dict { PACKAGE_PIN F22 IOSTANDARD LVCMOS18 } [get_ports { sw[0] }]
set_property -dict { PACKAGE_PIN G22 IOSTANDARD LVCMOS18 } [get_ports { sw[1] }]
set_property -dict { PACKAGE_PIN H22 IOSTANDARD LVCMOS18 } [get_ports { sw[2] }]
set_property -dict { PACKAGE_PIN F21 IOSTANDARD LVCMOS18 } [get_ports { sw[3] }]
set_property -dict { PACKAGE_PIN H19 IOSTANDARD LVCMOS18 } [get_ports { sw[4] }]
set_property -dict { PACKAGE_PIN H18 IOSTANDARD LVCMOS18 } [get_ports { sw[5] }]
set_property -dict { PACKAGE_PIN H17 IOSTANDARD LVCMOS18 } [get_ports { sw[6] }]
set_property -dict { PACKAGE_PIN M15 IOSTANDARD LVCMOS18 } [get_ports { sw[7] }]

# Push buttons (btnc, btnd, btnl, btnr, btnu)
set_property -dict { PACKAGE_PIN P16 IOSTANDARD LVCMOS18 } [get_ports { btn[0] }]
set_property -dict { PACKAGE_PIN R16 IOSTANDARD LVCMOS18 } [get_ports { btn[1] }]
set_property -dict { PACKAGE_PIN N15 IOSTANDARD LVCMOS18 } [get_ports { btn[2] }]
set_property -dict { PACKAGE_PIN R18 IOSTANDARD LVCMOS18 } [get_ports { btn[3] }]
set_property -dict { PACKAGE_PIN T18 IOSTANDARD LVCMOS18 } [get_ports { btn[4] }]

# LEDs
set_property -dict { PACKAGE_PIN T22 IOSTANDARD LVCMOS33 } [get_ports { led[0] }]
set_property -dict { PACKAGE_PIN T21 IOSTANDARD LVCMOS33 } [get_ports { led[1] }]
set_property -dict { PACKAGE_PIN U22 IOSTANDARD LVCMOS33 } [get_ports { led[2] }]
set_property -dict { PACKAGE_PIN U21 IOSTANDARD LVCMOS33 } [get_ports { led[3] }]
set_property -dict { PACKAGE_PIN V22 IOSTANDARD LVCMOS33 } [get_ports { led[4] }]
set_property -dict { PACKAGE_PIN W22 IOSTANDARD LVCMOS33 } [get_ports { led[5] }]
set_property -dict { PACKAGE_PIN U19 IOSTANDARD LVCMOS33 } [get_ports { led[6] }]
set_property -dict { PACKAGE_PIN U14 IOSTANDARD LVCMOS33 } [get_ports { led[7] }]

# Pmod JA
set_property -dict { PACKAGE_PIN Y11 IOSTANDARD LVCMOS33 } [get_ports { ja[0] }]
set_property -dict { PACKAGE_PIN AA11 IOSTANDARD LVCMOS33 } [get_ports { ja[1] }]
set_property -dict { PACKAGE_PIN Y10 IOSTANDARD LVCMOS33 } [get_ports { ja[2] }]
set_property -dict { PACKAGE_PIN AA9 IOSTANDARD LVCMOS33 } [get_ports { ja[3] }]
set_property -dict { PACKAGE_PIN AB11 IOSTANDARD LVCMOS33 } [get_ports { ja[4] }]
set_property -dict { PACKAGE_PIN AB10 IOSTANDARD LVCMOS33 } [get_ports { ja[5] }]
set_property -dict { PACKAGE_PIN AB9 IOSTANDARD LVCMOS33 } [get_ports { ja[6] }]
set_property -dict { PACKAGE_PIN AA8 IOSTANDARD LVCMOS33 } [get_ports { ja[7] }]

# Pmod JB
set_property -dict { PACKAGE_PIN W12 IOSTANDARD LVCMOS33 } [get_ports { jb[0] }]
set_property -dict { PACKAGE_PIN W11 IOSTANDARD LVCMOS33 } [get_ports { jb[1] }]
set_property -dict { PACKAGE_PIN V10 IOSTANDARD LVCMOS33 } [get_ports { jb[2] }]
set_property -dict { PACKAGE_PIN W8 IOSTANDARD LVCMOS33 } [get_ports { jb[3] }]
set_property -dict { PACKAGE_PIN V12 IOSTANDARD LVCMOS33 } [get_ports { jb[4] }]
set_property -dict { PACKAGE_PIN W10 IOSTANDARD LVCMOS33 } [get_ports { jb[5] }]
set_property -dict { PACKAGE_PIN V9 IOSTANDARD LVCMOS33 } [get_ports { jb[6] }]
set_property -dict { PACKAGE_PIN V8 IOSTANDARD LVCMOS33 } [get_ports { jb[7] }]

# Pmod JC
set_property -dict { PACKAGE_PIN AB7 IOSTANDARD LVCMOS33 } [get_ports { jc[0] }]
set_property -dict { PACKAGE_PIN AB6 IOSTANDARD LVCMOS33 } [get_ports { jc[1] }]
set_property -dict { PACKAGE_PIN Y4 IOSTANDARD LVCMOS33 } [get_ports { jc[2] }]
set_property -dict { PACKAGE_PIN AA4 IOSTANDARD LVCMOS33 } [get_ports { jc[3] }]
set_property -dict { PACKAGE_PIN R6 IOSTANDARD LVCMOS33 } [get_ports { jc[4] }]
set_property -dict { PACKAGE_PIN T6 IOSTANDARD LVCMOS33 } [get_ports { jc[5] }]
set_property -dict { PACKAGE_PIN T4 IOSTANDARD LVCMOS33 } [get_ports { jc[6] }]
set_property -dict { PACKAGE_PIN U4 IOSTANDARD LVCMOS33 } [get_ports { jc[7] }]

# Pmod JD
set_property -dict { PACKAGE_PIN V7 IOSTANDARD LVCMOS33 } [get_ports { jd[0] }]
set_property -dict { PACKAGE_PIN W7 IOSTANDARD LVCMOS33 } [get_ports { jd[1] }]
set_property -dict { PACKAGE_PIN V5 IOSTANDARD LVCMOS33 } [get_ports { jd[2] }]
set_property -dict { PACKAGE_PIN V4 IOSTANDARD LVCMOS33 } [get_ports { jd[3] }]
set_property -dict { PACKAGE_PIN W6 IOSTANDARD LVCMOS33 } [get_ports { jd[4] }]
set_property -dict { PACKAGE_PIN W5 IOSTANDARD LVCMOS33 } [get_ports { jd[5] }]
set_property -dict { PACKAGE_PIN U6 IOSTANDARD LVCMOS33 } [get_ports { jd[6] }]
set_property -dict { PACKAGE_PIN U5 IOSTANDARD LVCMOS33 } [get_ports { jd[7] }]
