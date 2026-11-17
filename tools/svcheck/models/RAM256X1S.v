// Behavioural model of the Xilinx 7-series RAM256X1S (256 x 1 LUT RAM, one port) for svcheck, written for Helion (not Xilinx's library).
module RAM256X1S #(parameter [255:0] INIT = 256'h0, parameter [0:0] IS_WCLK_INVERTED = 1'b0)
  (output O, input [7:0] A, input D, input WCLK, input WE);
  reg [255:0] m = INIT;
  wire clk = WCLK ^ IS_WCLK_INVERTED;
  always @(posedge clk) if (WE) m[A] <= D;
  assign O = m[A];
endmodule
