module top(input logic clk, input logic [3:0] a, output logic y);
  logic r = 1'b0;
  always_ff @(posedge clk) r <= (a != 4'd15);
  assign y = r;
endmodule
