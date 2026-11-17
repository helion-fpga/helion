// Behavioural model of the Xilinx 7-series RAM64M (four 64 x 1 LUT RAMs, port D writes) for svcheck, written for Helion (not Xilinx's library).
module RAM64M #(parameter [63:0] INIT_A = 64'h0, parameter [63:0] INIT_B = 64'h0, parameter [63:0] INIT_C = 64'h0,
                parameter [63:0] INIT_D = 64'h0, parameter [0:0] IS_WCLK_INVERTED = 1'b0)
  (output DOA, output DOB, output DOC, output DOD, input [5:0] ADDRA, input [5:0] ADDRB, input [5:0] ADDRC,
   input [5:0] ADDRD, input DIA, input DIB, input DIC, input DID, input WCLK, input WE);
  reg [63:0] a = INIT_A, b = INIT_B, c = INIT_C, d = INIT_D;
  wire clk = WCLK ^ IS_WCLK_INVERTED;
  always @(posedge clk) if (WE) begin a[ADDRD] <= DIA; b[ADDRD] <= DIB; c[ADDRD] <= DIC; d[ADDRD] <= DID; end
  assign DOA = a[ADDRA];
  assign DOB = b[ADDRB];
  assign DOC = c[ADDRC];
  assign DOD = d[ADDRD];
endmodule
