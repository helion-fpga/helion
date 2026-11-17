// Behavioural model of the Xilinx 7-series SRL16E for svcheck, written for Helion (not Xilinx's library).
module SRL16E #(parameter [15:0] INIT = 16'h0, parameter [0:0] IS_CLK_INVERTED = 1'b0)
  (input A0, input A1, input A2, input A3, input CE, input CLK, input D, output Q);
  reg [15:0] r = INIT;
  always @(posedge CLK) if (CE) r <= {r[14:0], D};
  assign Q = r[{A3, A2, A1, A0}];
endmodule
