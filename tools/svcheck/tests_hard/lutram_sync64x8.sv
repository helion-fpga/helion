// 64 x 8 with a registered read, too small for block RAM: LUT RAM (RAM64M)
// read into flops, with contents and a read enable.
module lutram_sync64x8 (input clk, input we, input re, input [5:0] wa, input [5:0] ra, input [7:0] wd, output logic [7:0] q);
  logic [7:0] mem [0:63];
  initial for (int i = 0; i < 64; i++) mem[i] = 8'(i * 151 + 7);
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= wd;
    if (re) q <= mem[ra];
  end
endmodule
