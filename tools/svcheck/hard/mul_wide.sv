// Products wider than one Mac27 (16 x 16 -> 32): built from several
// blocks, one per 16 x 16 partial product, and adders.
module mul_wide(input logic clk, input logic [23:0] a, input logic [23:0] b, input logic [31:0] x, input logic [31:0] y,
                input logic signed [19:0] sa, input logic signed [19:0] sb, input logic [15:0] u, input logic signed [15:0] s,
                output logic [47:0] p48, output logic [31:0] p32, output logic signed [39:0] ps, output logic [63:0] p64,
                output logic signed [31:0] pm, output logic [47:0] pr, output logic [39:0] pu);
  assign p48 = a * b;
  assign p32 = x * y;
  assign ps = sa * sb;
  assign p64 = {32'd0, x} * {32'd0, y};
  assign pm = s * $signed({1'b0, u});
  assign pu = {16'd0, u} * {16'd0, u ^ 16'h1234};
  always_ff @(posedge clk) pr <= a * {b[11:0], b[23:12]};
endmodule
