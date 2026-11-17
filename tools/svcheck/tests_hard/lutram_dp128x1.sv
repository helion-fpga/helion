// 128 x 1, read at the write address and at a second address: RAM128X1D
// (four LUTs and the slice's F7 muxes), with contents.
module lutram_dp128x1 (input clk, input we, input [6:0] wa, input [6:0] ra, input d, output qw, output qr);
  logic mem [0:127];
  initial for (int i = 0; i < 128; i++) mem[i] = 1'((i * 13 + 3) >> 3);
  always_ff @(posedge clk) if (we) mem[wa] <= d;
  assign qw = mem[wa];
  assign qr = mem[ra];
endmodule
