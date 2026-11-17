// Asynchronous read: stays in flops and LUTs.
module async_rd (input clk, input we, input [6:0] a, input [15:0] d, output [15:0] q);
  logic [15:0] mem [0:127];
  always_ff @(posedge clk) if (we) mem[a] <= d;
  assign q = mem[a];
endmodule
