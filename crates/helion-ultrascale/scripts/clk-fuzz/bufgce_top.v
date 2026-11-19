// BUFGCE fuzz top (Job 4, sub-pass 1).
// Baseline: CE_TYPE SYNC, nothing inverted. The driver changes one
// attribute per variant run (IS_CE_INVERTED, IS_I_INVERTED, CE_TYPE).
module clk_fuzz_top (
  input wire clk_in,
  input wire ce,
  output wire clk_out
);
  (* DONT_TOUCH = "yes" *)
  BUFGCE #(
    .CE_TYPE("SYNC"),
    .IS_CE_INVERTED(1'b0),
    .IS_I_INVERTED(1'b0)
  ) bufgce_inst (
    .I(clk_in),
    .CE(ce),
    .O(clk_out)
  );
endmodule
