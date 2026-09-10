# Helion SDC — dual clocks + pin-scoped set_false_path → CDC-14 Warning
create_clock -period 10.000 [get_ports clk_a]
create_clock -period 8.000 [get_ports clk_b]
# Mixed clock↔pin exception: not full clock-to-clock False Path (CDC-15 Safe).
set_false_path -from [get_clocks clk_a] -to [get_pins q_b/D]
set_false_path -from [get_clocks clk_b] -to [get_pins q_a/D]
