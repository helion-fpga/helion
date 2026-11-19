# Clock constraint for the CLK fuzz tops. Every top exposes its clock
# input as clk_in; the driver applies this XDC to each run. The 10 ns
# period is a placeholder that keeps place/route happy; it is not a
# timing target and does not affect the config-bit diff.
create_clock -period 10.000 [get_ports clk_in]
