# ZedBoard (xc7z020clg484-1) pins for examples/zedboard_ps7_axi.sv.
# Pin names are from the Digilent ZedBoard master XDC.

# FCLK_CLK0 from the PS, 100 MHz (the frequency the PS boot code sets).
create_clock -period 10.000 -name fclk0 [get_pins ps7/FCLKCLK[0]]

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
