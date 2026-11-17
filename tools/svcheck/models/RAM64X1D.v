// Behavioural model of the Xilinx 7-series RAM64X1D (64 x 1 LUT RAM, write port and a second read port) for svcheck, written for Helion (not Xilinx's library).
module RAM64X1D #(parameter [63:0] INIT = 64'h0, parameter [0:0] IS_WCLK_INVERTED = 1'b0)
  (output SPO, output DPO, input DPRA0, input DPRA1, input DPRA2, input DPRA3, input DPRA4, input DPRA5, input A0, input A1, input A2, input A3, input A4, input A5, input D, input WCLK, input WE);
  reg [63:0] m = INIT;
  wire clk = WCLK ^ IS_WCLK_INVERTED;
  always @(posedge clk) if (WE) m[{A5, A4, A3, A2, A1, A0}] <= D;
  assign SPO = m[{A5, A4, A3, A2, A1, A0}];
  assign DPO = m[{DPRA5, DPRA4, DPRA3, DPRA2, DPRA1, DPRA0}];
endmodule
