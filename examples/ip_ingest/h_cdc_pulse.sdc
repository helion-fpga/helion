# 10 ns period — same class as gold counter; h_cdc_pulse dual-clock
create_clock -period 10.000 -name src_clk [get_ports src_clk]
create_clock -period 10.000 -name dst_clk [get_ports dst_clk]
