set_property -dict {PACKAGE_PIN Y9 IOSTANDARD LVCMOS33} [get_ports clk]
set_property -dict {PACKAGE_PIN T22 IOSTANDARD LVCMOS33} [get_ports o]
create_clock -period 10.000 [get_ports clk]
