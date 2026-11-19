// BUFGCTRL fuzz top (Job 4, sub-pass 2).
// Baseline: nothing inverted, I0 preselected. The driver changes one
// attribute per variant run (IS_CE0/IS_CE1/IS_I0/IS_I1/IS_IGNORE0/
// IS_IGNORE1/IS_S0/IS_S1_INVERTED, PRESELECT_I0, PRESELECT_I1).
module clk_fuzz_top (
  input wire clk_in,
  input wire clk_in1,
  input wire ce0,
  input wire ce1,
  input wire s0,
  input wire s1,
  input wire ignore0,
  input wire ignore1,
  output wire clk_out
);
  (* DONT_TOUCH = "yes" *)
  BUFGCTRL #(
    .INIT_OUT(1'b0),
    .IS_CE0_INVERTED(1'b0),
    .IS_CE1_INVERTED(1'b0),
    .IS_I0_INVERTED(1'b0),
    .IS_I1_INVERTED(1'b0),
    .IS_IGNORE0_INVERTED(1'b0),
    .IS_IGNORE1_INVERTED(1'b0),
    .IS_S0_INVERTED(1'b0),
    .IS_S1_INVERTED(1'b0),
    .PRESELECT_I0("TRUE"),
    .PRESELECT_I1("FALSE")
  ) bufgctrl_inst (
    .I0(clk_in),
    .I1(clk_in1),
    .CE0(ce0),
    .CE1(ce1),
    .S0(s0),
    .S1(s1),
    .IGNORE0(ignore0),
    .IGNORE1(ignore1),
    .O(clk_out)
  );
endmodule
