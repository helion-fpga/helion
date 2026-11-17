// 256 x 1 single port: RAM256X1S (four LUTs, F7 and F8 muxes), with contents.
module lutram_sp256x1 (input clk, input we, input [7:0] a, input d, output q);
  logic mem [0:255];
  initial for (int i = 0; i < 256; i++) mem[i] = 1'((i * 29 + 11) >> 4);
  always_ff @(posedge clk) if (we) mem[a] <= d;
  assign q = mem[a];
endmodule
