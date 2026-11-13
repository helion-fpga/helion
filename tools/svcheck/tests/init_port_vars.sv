// Ports that are variables take a power-up value from an initial block:
// non-ANSI outputs redeclared as reg, and an ANSI 'output reg' of a
// submodule. (An initial block assigning a net, such as a wire or an
// output with no data type, is an error.)
module init_port_vars_sub(input logic clk, input logic [3:0] d, output reg [3:0] s);
  initial s = 4'ha;
  always_ff @(posedge clk) s <= s ^ d;
endmodule

module init_port_vars(clk, d, q, k, s);
  input clk;
  input [7:0] d;
  output [7:0] q;
  reg [7:0] q;
  output [3:0] k;
  reg [3:0] k;
  output [3:0] s;
  initial q = 8'h3c;
  initial k = 4'd9;
  always @(posedge clk) q <= q + d;
  init_port_vars_sub u(.clk(clk), .d(d[3:0]), .s(s));
endmodule
