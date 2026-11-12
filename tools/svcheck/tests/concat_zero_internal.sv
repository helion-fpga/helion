// `{bit, 3'b000}` assigned to an internal wire (not an output).
module top(input logic [3:0] a, input logic b, output logic [3:0] y);
  wire c = a[1];
  wire [3:0] t = {c, 3'b000};
  wire [3:0] r = {b, b, b, b};
  wire [3:0] u = t & r;
  assign y = u;
endmodule
