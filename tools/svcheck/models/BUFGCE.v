// Behavioural model of the Xilinx 7-series BUFGCE for svcheck, written for Helion (not Xilinx's library).
// CE is sampled while I is low, so the output never glitches.
module BUFGCE #(parameter CE_TYPE = "SYNC", parameter [0:0] IS_CE_INVERTED = 1'b0, parameter [0:0] IS_I_INVERTED = 1'b0) (input I, input CE, output O);
  reg en = 1'b0;
  always @* if (!I) en = CE;
  assign O = I & en;
endmodule
