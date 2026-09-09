# Helion SDC — 100 MHz on clk (Ibex pin-wrap user clock)
create_clock -period 10.000 [get_ports clk]
# Gold HAD IOB sites (same as counter/pico/SERV): clk + led on unused package pins.
set_property PACKAGE_PIN IOB_X3Y0 [get_ports clk]
set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]
