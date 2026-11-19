// PLLE4_ADV fuzz top (Job 4, sub-pass 6).
// Baseline values below are this top's chosen baseline, recorded in
// runs.txt; they are not claimed to be Vivado defaults. The driver
// enumerates the placed cell's properties at runtime and fuzzes each
// one found. The PLL output drives a BUFGCE so the clock reaches the
// fabric and is not optimized away.
module clk_fuzz_top (
  input wire clk_in,
  input wire rst,
  output wire clk_out,
  output wire locked
);
  wire clkfb;
  wire clk0;
  (* DONT_TOUCH = "yes" *)
  PLLE4_ADV #(
    .CLKFBOUT_MULT(5),
    .CLKFBOUT_PHASE(0.0),
    .CLKIN_PERIOD(10.0),
    .CLKOUT0_DIVIDE(5),
    .CLKOUT0_DUTY_CYCLE(0.5),
    .CLKOUT0_PHASE(0.0),
    .DIVCLK_DIVIDE(1),
    .COMPENSATION("ZHOLD"),
    .STARTUP_WAIT("FALSE")
  ) pll_inst (
    .CLKIN(clk_in),
    .CLKFBIN(clkfb),
    .CLKFBOUT(clkfb),
    .CLKOUT0(clk0),
    .LOCKED(locked),
    .PWRDWN(1'b0),
    .RST(rst)
  );
  (* DONT_TOUCH = "yes" *)
  BUFGCE bufg_out (
    .I(clk0),
    .CE(1'b1),
    .O(clk_out)
  );
endmodule
