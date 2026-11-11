// Asynchronous reset (active-high in the child, active-low at the top) and
// asynchronous set values, through a module boundary.
module cnt3(input logic clk, input logic rst, input logic en, output logic [2:0] q);
  always_ff @(posedge clk or posedge rst)
    if (rst) q <= 3'b101;
    else if (en) q <= q + 3'd1;
endmodule
module top(input logic clk, input logic rst_n, input logic en, input logic d, output logic [2:0] q, output logic r);
  cnt3 u(.clk(clk), .rst(~rst_n), .en(en), .q(q));
  always_ff @(posedge clk or negedge rst_n)
    if (!rst_n) r <= 1'b1;
    else r <= d;
endmodule
