// BUFGCE_DIV fuzz top (Job 4, sub-pass 3).
// Baseline: divide by 1, CE_TYPE SYNC, nothing inverted. The driver
// changes one attribute per variant run (IS_CE/IS_I/IS_CLR_INVERTED,
// CE_TYPE, BUFGCE_DIVIDE 2..8).
module clk_fuzz_top (
  input wire clk_in,
  input wire ce,
  input wire clr,
  output wire clk_out
);
  (* DONT_TOUCH = "yes" *)
  BUFGCE_DIV #(
    .BUFGCE_DIVIDE(1),
    .CE_TYPE("SYNC"),
    .IS_CE_INVERTED(1'b0),
    .IS_CLR_INVERTED(1'b0),
    .IS_I_INVERTED(1'b0)
  ) bufgce_div_inst (
    .I(clk_in),
    .CE(ce),
    .CLR(clr),
    .O(clk_out)
  );
endmodule
