# Helion SDC — dual clocks + set_max_delay -datapath_only → CDC-13 Warning
create_clock -period 10.000 [get_ports clk_a]
create_clock -period 8.000 [get_ports clk_b]
set_max_delay -datapath_only 2.0 -from [get_clocks clk_a] -to [get_clocks clk_b]
set_max_delay -datapath_only 2.0 -from [get_clocks clk_b] -to [get_clocks clk_a]
