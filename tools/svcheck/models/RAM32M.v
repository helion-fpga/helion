// Behavioural model of the Xilinx 7-series RAM32M (four 32 x 2 LUT RAMs, port D writes) for svcheck, written for Helion (not Xilinx's library).
module RAM32M #(parameter [63:0] INIT_A = 64'h0, parameter [63:0] INIT_B = 64'h0, parameter [63:0] INIT_C = 64'h0,
                parameter [63:0] INIT_D = 64'h0, parameter [0:0] IS_WCLK_INVERTED = 1'b0)
  (output [1:0] DOA, output [1:0] DOB, output [1:0] DOC, output [1:0] DOD, input [4:0] ADDRA, input [4:0] ADDRB,
   input [4:0] ADDRC, input [4:0] ADDRD, input [1:0] DIA, input [1:0] DIB, input [1:0] DIC, input [1:0] DID, input WCLK, input WE);
  reg [63:0] a = INIT_A, b = INIT_B, c = INIT_C, d = INIT_D;
  wire clk = WCLK ^ IS_WCLK_INVERTED;
  always @(posedge clk) if (WE) begin
    a[{ADDRD, 1'b0} +: 2] <= DIA; b[{ADDRD, 1'b0} +: 2] <= DIB; c[{ADDRD, 1'b0} +: 2] <= DIC; d[{ADDRD, 1'b0} +: 2] <= DID;
  end
  assign DOA = a[{ADDRA, 1'b0} +: 2];
  assign DOB = b[{ADDRB, 1'b0} +: 2];
  assign DOC = c[{ADDRC, 1'b0} +: 2];
  assign DOD = d[{ADDRD, 1'b0} +: 2];
endmodule
