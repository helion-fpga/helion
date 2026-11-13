// Registered products written through a wire (P register), and a
// register named like a multiplier's product net.
module mac_wire(input logic clk, input logic t, input logic [15:0] a, input logic [15:0] b, input logic [15:0] x, input logic [15:0] y,
                output logic [31:0] r, output logic [31:0] p, output logic u_mul1_p3);
  logic [31:0] m;
  assign m = a * b;
  always_ff @(posedge clk) r <= m;
  assign p = x * y;
  always_ff @(posedge clk) u_mul1_p3 <= t;
endmodule
