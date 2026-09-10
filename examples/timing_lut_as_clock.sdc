# Helion SDC — primary clock on clk; gated net is LUT-as-clock (TIMING-10)
create_clock -period 10.000 [get_ports clk]
set_input_delay -clock clk 1.0 [get_ports en]
set_input_delay -clock clk 1.0 [get_ports d]
set_output_delay -clock clk 1.0 [get_ports q]
set_clock_uncertainty 0.1 [get_clocks clk]
