# LFE5U-85F CABGA381: clock on A10 (a primary clock input), LED on A12
create_clock -period 10.000 [get_ports clk]
set_property PACKAGE_PIN A10 [get_ports clk]
set_property PACKAGE_PIN A12 [get_ports led]
