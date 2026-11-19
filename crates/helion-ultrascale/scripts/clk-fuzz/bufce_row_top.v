// BUFCE_ROW fuzz top (Job 4, sub-pass 4).
// No public source names the BUFCE_ROW primitive attributes, so this
// top instantiates the primitive with its documented ports only and
// the driver enumerates the placed cell's properties at runtime. If
// Vivado rejects BUFCE_ROW as a primitive name, that is logged in
// runs.txt and the sub-pass is reported as not runnable, not faked.
module clk_fuzz_top (
  input wire clk_in,
  input wire ce,
  output wire clk_out
);
  (* DONT_TOUCH = "yes" *)
  BUFCE_ROW bufce_row_inst (
    .I(clk_in),
    .CE(ce),
    .O(clk_out)
  );
endmodule
