// Behavioural model of the Xilinx 7-series RAM32X1D (32 x 1 LUT RAM, write port and a second read port) for svcheck, written for Helion (not Xilinx's library).
module RAM32X1D #(parameter [31:0] INIT = 32'h0, parameter [0:0] IS_WCLK_INVERTED = 1'b0)
  (output SPO, output DPO, input DPRA0, input DPRA1, input DPRA2, input DPRA3, input DPRA4, input A0, input A1, input A2, input A3, input A4, input D, input WCLK, input WE);
  reg [31:0] m = INIT;
  wire clk = WCLK ^ IS_WCLK_INVERTED;
  always @(posedge clk) if (WE) m[{A4, A3, A2, A1, A0}] <= D;
  assign SPO = m[{A4, A3, A2, A1, A0}];
  assign DPO = m[{DPRA4, DPRA3, DPRA2, DPRA1, DPRA0}];
endmodule
