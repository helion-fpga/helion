module top(input logic clk_a, input logic clk_b, input logic d, output logic q, output logic r);
  logic div;
  always_ff @(posedge clk_a) begin q <= d; div <= ~div; end
  always_ff @(posedge clk_b) r <= q;
endmodule
