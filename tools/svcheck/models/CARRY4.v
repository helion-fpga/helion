// Behavioural model of the Xilinx 7-series CARRY4 for svcheck, written for Helion (not Xilinx's library).
module CARRY4 (input CI, input CYINIT, input [3:0] DI, input [3:0] S, output [3:0] CO, output [3:0] O);
  wire [4:0] c;
  assign c[0] = CI | CYINIT;
  genvar i;
  for (i = 0; i < 4; i = i + 1) begin : g
    assign c[i + 1] = S[i] ? c[i] : DI[i];
    assign O[i] = S[i] ^ c[i];
  end
  assign CO = c[4:1];
endmodule
