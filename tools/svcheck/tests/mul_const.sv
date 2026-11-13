// Products with a constant factor: shift-and-add over canonical signed
// digits (a run of ones is one addition and one subtraction). Unsigned and
// signed, negative constants, constants with many set bits (runs, and
// alternating bits that no signed digit shortens), several widths, and a
// truncated and a registered product.
module mul_const(input logic clk, input logic [7:0] a, input logic [11:0] b, input logic signed [9:0] sa,
                 input logic signed [15:0] sb, input logic [23:0] c, input logic [15:0] d, input logic [31:0] e,
                 input logic signed [6:0] sc,
                 output logic [15:0] p0, output logic [23:0] p1, output logic [23:0] p2, output logic signed [15:0] p3,
                 output logic signed [31:0] p4, output logic [47:0] p5, output logic [31:0] p6, output logic [31:0] p7,
                 output logic signed [13:0] p8, output logic [19:0] p9, output logic signed [23:0] p10, output logic [11:0] p11);
  assign p0 = a * 8'd123;
  assign p1 = b * 12'hfff;
  assign p2 = b * 24'd255;
  assign p3 = sa * -16'sd7;
  assign p4 = sb * -32'sd12345;
  assign p5 = c * 48'h5555_5555;
  assign p6 = d * 32'hf0f0;
  assign p7 = e * 32'd1000003;
  assign p8 = sc * -14'sd64;
  assign p10 = sb * 24'sh7ff_bff;
  assign p11 = b * 12'hf7f;
  always_ff @(posedge clk) p9 <= {8'd0, b} * 20'd4095;
endmodule
