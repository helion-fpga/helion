# Helion SDC — dual clocks for CDC-1 Warning demo (2FF synchronizer)
create_clock -period 10.000 [get_ports clk_a]
create_clock -period 8.000 [get_ports clk_b]
