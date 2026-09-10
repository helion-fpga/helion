# Helion SDC — dual clocks + async groups → CDC-15 Info (not Critical)
create_clock -period 10.000 [get_ports clk_a]
create_clock -period 8.000 [get_ports clk_b]
set_clock_groups -asynchronous -group [get_clocks clk_a] -group [get_clocks clk_b]
