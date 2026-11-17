# ZedBoard (xc7z020clg484-1) pins for examples/zedboard_bram_fill.sv, from
# the Digilent ZedBoard master XDC (as in examples/zedboard_pico_soc.xdc).
# BTNC is on a Vadj bank: LVCMOS18 matches the factory jumper (J18 at 1.8 V).

set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]

# BTNC runs the test again (it also starts by itself after configuration).
set_property PACKAGE_PIN P16 [get_ports {rst}]
set_property IOSTANDARD LVCMOS18 [get_ports {rst}]

# LD7 done, LD6 passed, LD0 heartbeat (passed) or LD0..LD2 failed scans.
set_property PACKAGE_PIN T22 [get_ports {led[0]}]
set_property PACKAGE_PIN T21 [get_ports {led[1]}]
set_property PACKAGE_PIN U22 [get_ports {led[2]}]
set_property PACKAGE_PIN U21 [get_ports {led[3]}]
set_property PACKAGE_PIN V22 [get_ports {led[4]}]
set_property PACKAGE_PIN W22 [get_ports {led[5]}]
set_property PACKAGE_PIN U19 [get_ports {led[6]}]
set_property PACKAGE_PIN U14 [get_ports {led[7]}]
set_property IOSTANDARD LVCMOS33 [get_ports {led[*]}]
