// Simple dual port: one write port, one read port with an enable.
module sdp (input clk, input we, input re, input [10:0] wa, input [10:0] ra, input [7:0] wd, output logic [7:0] q);
  logic [7:0] mem [0:2047];
  always_ff @(posedge clk) if (we) mem[wa] <= wd;
  always_ff @(posedge clk) if (re) q <= mem[ra];
endmodule
