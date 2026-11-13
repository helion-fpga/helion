// Signed products wider than one Mac27, on signed partial products: an
// operand sign-extended from n bits is ceil(n/16) pieces, the top one
// signed, so a product takes ceil(n/16) * ceil(m/16) blocks (20 x 20 and
// 24 x 24: 4, 32 x 32 with 32 bits of result: 3, 31 x 17: 4, 40 x 12: 3,
// signed 16 x unsigned 16: 1). Also a registered one and a signed
// product of zero-extended operands (unsigned pieces).
module mul_signed_wide(input logic clk, input logic signed [19:0] a20, input logic signed [19:0] b20,
                       input logic signed [23:0] a24, input logic signed [23:0] b24,
                       input logic signed [31:0] a32, input logic signed [31:0] b32,
                       input logic signed [30:0] a31, input logic signed [16:0] b17,
                       input logic signed [39:0] a40, input logic signed [11:0] b12,
                       input logic signed [15:0] s16, input logic [15:0] u16,
                       output logic signed [39:0] p20, output logic signed [47:0] p24, output logic signed [31:0] p32,
                       output logic signed [47:0] p31, output logic signed [51:0] p40, output logic signed [31:0] pm,
                       output logic signed [47:0] pr, output logic signed [33:0] pz);
  assign p20 = a20 * b20;
  assign p24 = a24 * b24;
  assign p32 = a32 * b32;
  assign p31 = a31 * b17;
  assign p40 = a40 * b12;
  assign pm = s16 * $signed({1'b0, u16});
  assign pz = $signed({1'b0, a20[15:0]}) * $signed({1'b0, b20[16:0]});
  always_ff @(posedge clk) pr <= a24 * b24;
endmodule
