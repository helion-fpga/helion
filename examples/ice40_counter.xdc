# iCE40-HX8K breakout board (CT256): 12 MHz oscillator on J3, LED0 on B5
create_clock -period 83.333 [get_ports clk]
set_property PACKAGE_PIN J3 [get_ports clk]
set_property PACKAGE_PIN B5 [get_ports led]
