# Helion SDC — dual clocks for CDC-10 Critical demo (no set_clock_groups)
create_clock -period 10.000 [get_ports clk_a]
create_clock -period 8.000 [get_ports clk_b]
