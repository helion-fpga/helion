// Registers with an asynchronous reset that an initial block also gives a
// power-up value other than the reset value: the reset still drives the
// RTL reset value. Active-low and active-high resets; bits where the two
// values agree and bits where they differ; a register with a declaration
// initializer next to them.
module init_async_reset(input logic clk, input logic arst_n, input logic rst, input logic [7:0] d,
                        output logic [7:0] q_lo, output logic [7:0] q_hi, output logic [7:0] q_decl);
  logic [7:0] r_lo;
  logic [7:0] r_hi;
  logic [7:0] r_decl = 8'h5a;
  initial r_lo = 8'hc3;
  initial r_hi = 8'hf0;
  always_ff @(posedge clk or negedge arst_n)
    if (!arst_n) r_lo <= 8'h02;
    else r_lo <= r_lo ^ d;
  always_ff @(posedge clk or posedge rst)
    if (rst) r_hi <= 8'h0f;
    else r_hi <= r_hi + d;
  always_ff @(posedge clk or negedge arst_n)
    if (!arst_n) r_decl <= 8'h81;
    else r_decl <= {r_decl[6:0], r_decl[7] ^ d[0]};
  assign q_lo = r_lo;
  assign q_hi = r_hi;
  assign q_decl = r_decl;
endmodule
