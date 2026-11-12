module top(input logic clk, input logic [3:0] a, output logic [3:0] q);
  logic [3:0] r = 4'd0;
  always_ff @(posedge clk) r <= (r != 4'd15) ? a : r;
  assign q = r;
endmodule
