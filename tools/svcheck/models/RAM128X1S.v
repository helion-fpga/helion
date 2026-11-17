// Behavioural model of the Xilinx 7-series RAM128X1S (128 x 1 LUT RAM, one port) for svcheck, written for Helion (not Xilinx's library).
module RAM128X1S #(parameter [127:0] INIT = 128'h0, parameter [0:0] IS_WCLK_INVERTED = 1'b0)
  (output O, input A0, input A1, input A2, input A3, input A4, input A5, input A6, input D, input WCLK, input WE);
  reg [127:0] m = INIT;
  wire clk = WCLK ^ IS_WCLK_INVERTED;
  always @(posedge clk) if (WE) m[{A6, A5, A4, A3, A2, A1, A0}] <= D;
  assign O = m[{A6, A5, A4, A3, A2, A1, A0}];
endmodule
