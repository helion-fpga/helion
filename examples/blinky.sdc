# User clock + pin LOC for the blinky lab (not the empty-XDC gold path).
create_clock -period 10.000 [get_ports clk]
set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]
