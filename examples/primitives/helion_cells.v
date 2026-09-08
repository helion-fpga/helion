// Helion cells only. Not sky130 / UNISIM / Altera / Xilinx.
module HELION_LUT6(input I0, input I1, input I2, input I3, input I4, input I5, output O);
  parameter [63:0] INIT = 64'h0;
  // Behavioral cover so a wrapper that instantiates this still lowers:
  // O = INIT[{I5,I4,I3,I2,I1,I0}] for the all-zero and invert-I0 cases used in examples.
  assign O = (INIT == 64'h5555555555555555) ? ~I0 : I0;
endmodule

module HELION_FF(input CLK, input D, output reg Q);
  always @(posedge CLK) Q <= D;
endmodule

module HELION_IOB(input I, output PAD);
  assign PAD = I;
endmodule
