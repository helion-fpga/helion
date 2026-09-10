# Helion SDC — dual clocks + exclusive groups → CDC-16 Info (not Critical)
create_clock -period 10.000 [get_ports clk_a]
create_clock -period 8.000 [get_ports clk_b]
set_clock_groups -exclusive -group [get_clocks clk_a] -group [get_clocks clk_b]
