// Behavioural model of the Xilinx 7-series RAM128X1D (128 x 1 LUT RAM, write port and a second read port) for svcheck, written for Helion (not Xilinx's library).
module RAM128X1D #(parameter [127:0] INIT = 128'h0, parameter [0:0] IS_WCLK_INVERTED = 1'b0)
  (output SPO, output DPO, input [6:0] DPRA, input [6:0] A, input D, input WCLK, input WE);
  reg [127:0] m = INIT;
  wire clk = WCLK ^ IS_WCLK_INVERTED;
  always @(posedge clk) if (WE) m[A] <= D;
  assign SPO = m[A];
  assign DPO = m[DPRA];
endmodule
