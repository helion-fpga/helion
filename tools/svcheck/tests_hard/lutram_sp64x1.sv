// 64 x 1 single port, read at the write address: one LUT (RAM64X1S).
module lutram_sp64x1 (input clk, input we, input [5:0] a, input d, output q);
  logic mem [0:63];
  initial for (int i = 0; i < 64; i++) mem[i] = 1'(64'hC3A5_96F0_1E2D_4B78 >> i);
  always_ff @(posedge clk) if (we) mem[a] <= d;
  assign q = mem[a];
endmodule
