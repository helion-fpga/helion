# ZedBoard (xc7z020clg484-1) pins for examples/zedboard_mmcm.sv.
# Pin names are from the Digilent ZedBoard master XDC.

set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]

# The three MMCM outputs only meet through the two-flop synchronizers of
# the handshake, so they are timed separately (both Vivado and zedbit
# read this line; zedbit still refuses a crossing with logic between the
# two flops).
set_clock_groups -asynchronous -group [get_clocks -of_objects [get_pins u_mmcm/CLKOUT0]] -group [get_clocks -of_objects [get_pins u_mmcm/CLKOUT1]] -group [get_clocks -of_objects [get_pins u_mmcm/CLKOUT2]]

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

# Pmod JA (dbg[7:0]) and JB (dbg[15:8]): debug state for co-simulation.
set_property PACKAGE_PIN Y11 [get_ports {dbg[0]}]
set_property PACKAGE_PIN AA11 [get_ports {dbg[1]}]
set_property PACKAGE_PIN Y10 [get_ports {dbg[2]}]
set_property PACKAGE_PIN AA9 [get_ports {dbg[3]}]
set_property PACKAGE_PIN AB11 [get_ports {dbg[4]}]
set_property PACKAGE_PIN AB10 [get_ports {dbg[5]}]
set_property PACKAGE_PIN AB9 [get_ports {dbg[6]}]
set_property PACKAGE_PIN AA8 [get_ports {dbg[7]}]
set_property PACKAGE_PIN W12 [get_ports {dbg[8]}]
set_property PACKAGE_PIN W11 [get_ports {dbg[9]}]
set_property PACKAGE_PIN V10 [get_ports {dbg[10]}]
set_property PACKAGE_PIN W8 [get_ports {dbg[11]}]
set_property PACKAGE_PIN V12 [get_ports {dbg[12]}]
set_property PACKAGE_PIN W10 [get_ports {dbg[13]}]
set_property PACKAGE_PIN V9 [get_ports {dbg[14]}]
set_property PACKAGE_PIN V8 [get_ports {dbg[15]}]
set_property IOSTANDARD LVCMOS33 [get_ports {dbg[*]}]
