// Behavioural model of the Xilinx 7-series SRLC32E for svcheck, written for Helion (not Xilinx's library).
module SRLC32E #(parameter [31:0] INIT = 32'h0, parameter [0:0] IS_CLK_INVERTED = 1'b0)
  (input [4:0] A, input CE, input CLK, input D, output Q, output Q31);
  reg [31:0] r = INIT;
  always @(posedge CLK) if (CE) r <= {r[30:0], D};
  assign Q = r[A];
  assign Q31 = r[31];
endmodule
