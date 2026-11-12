module ch(input logic clk, input logic rst, input logic d, output logic q);
  always_ff @(posedge clk) if (rst) q <= 1'b1; else q <= d;
endmodule
module top(input logic clk, input logic rst_n, input logic d, output logic q);
  ch u(.clk(clk), .rst(~rst_n), .d(d), .q(q));
endmodule
