// 32 x 8 register file, two asynchronous reads and one write: LUT RAM
// (RAM32M), with contents.
module lutram_rf32x8 (input clk, input we, input [4:0] wa, input [7:0] wd,
                      input [4:0] ra1, input [4:0] ra2, output [7:0] rd1, output [7:0] rd2);
  logic [7:0] rf [0:31];
  initial for (int i = 0; i < 32; i++) rf[i] = 8'(i * 37 + 5);
  always_ff @(posedge clk) if (we) rf[wa] <= wd;
  assign rd1 = rf[ra1];
  assign rd2 = rf[ra2];
endmodule
