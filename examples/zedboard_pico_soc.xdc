# ZedBoard (xc7z020clg484-1) pins for examples/zedboard_pico_soc.sv.
# Pin names are from the Digilent ZedBoard master XDC. The switches and
# buttons are on the Vadj banks: LVCMOS18 matches the factory jumper setting
# (J18 at 1.8 V); use LVCMOS33 there if J18 is set to 3.3 V.

set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]

# BTNC restarts the program.
set_property PACKAGE_PIN P16 [get_ports {rst}]
set_property IOSTANDARD LVCMOS18 [get_ports {rst}]

# SW0..SW4: lowest counter bit shown on the LEDs.
set_property PACKAGE_PIN F22 [get_ports {sw[0]}]
set_property PACKAGE_PIN G22 [get_ports {sw[1]}]
set_property PACKAGE_PIN H22 [get_ports {sw[2]}]
set_property PACKAGE_PIN F21 [get_ports {sw[3]}]
set_property PACKAGE_PIN H19 [get_ports {sw[4]}]
set_property IOSTANDARD LVCMOS18 [get_ports {sw[*]}]

# LD0..LD7.
set_property PACKAGE_PIN T22 [get_ports {led[0]}]
set_property PACKAGE_PIN T21 [get_ports {led[1]}]
set_property PACKAGE_PIN U22 [get_ports {led[2]}]
set_property PACKAGE_PIN U21 [get_ports {led[3]}]
set_property PACKAGE_PIN V22 [get_ports {led[4]}]
set_property PACKAGE_PIN W22 [get_ports {led[5]}]
set_property PACKAGE_PIN U19 [get_ports {led[6]}]
set_property PACKAGE_PIN U14 [get_ports {led[7]}]
set_property IOSTANDARD LVCMOS33 [get_ports {led[*]}]
