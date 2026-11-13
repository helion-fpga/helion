// Multiplies: unsigned 16 x 16 -> 32, signed 16 x 16 -> 32, an 8 x 8
// product, and one registered. With HELION_V2_HARD=1 each is a Mac27.
module mul(input logic clk, input logic [15:0] a, input logic [15:0] b, input logic signed [15:0] sa,
           input logic signed [15:0] sb, input logic [7:0] c, input logic [7:0] e,
           output logic [31:0] pu, output logic signed [31:0] ps, output logic [15:0] p8, output logic [31:0] pr);
  assign pu = a * b;
  assign ps = sa * sb;
  assign p8 = c * e;
  always_ff @(posedge clk) pr <= (a ^ 16'h5a5a) * b + 32'd7;
endmodule
