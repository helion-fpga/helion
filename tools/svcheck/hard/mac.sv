// Multiplies a Mac27 takes whole: registered (PREG=1, its enable and
// synchronous reset on CE and RST), plus an addend on C, combinational
// with C, and signed. With HELION_V2_HARD=1 each is one Mac27.
module mac(input logic clk, input logic en, input logic rst, input logic [15:0] a, input logic [15:0] b, input logic [31:0] c,
           input logic signed [15:0] sa, input logic signed [15:0] sb, input logic signed [31:0] sc,
           output logic [31:0] p1, output logic [31:0] p2, output logic [31:0] p3, output logic signed [31:0] ps);
  always_ff @(posedge clk) p1 <= a * b;
  always_ff @(posedge clk) if (rst) p2 <= '0; else if (en) p2 <= a * b + c;
  assign p3 = a * b + c;
  always_ff @(posedge clk) if (en) ps <= sa * sb + sc;
endmodule
