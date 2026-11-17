// Minimal behavioural models of the UNISIM primitives Helion's Vivado export
// writes, written from the documented behaviour (UG953) for zero-delay
// co-simulation only. Not Xilinx's models: no timing, no X checks, and only
// the parameters the export sets.

module LUT1 #(parameter [1:0] INIT = 0) (output O, input I0);
  assign O = INIT[I0];
endmodule
module LUT2 #(parameter [3:0] INIT = 0) (output O, input I0, I1);
  assign O = INIT[{I1, I0}];
endmodule
module LUT3 #(parameter [7:0] INIT = 0) (output O, input I0, I1, I2);
  assign O = INIT[{I2, I1, I0}];
endmodule
module LUT4 #(parameter [15:0] INIT = 0) (output O, input I0, I1, I2, I3);
  assign O = INIT[{I3, I2, I1, I0}];
endmodule
module LUT5 #(parameter [31:0] INIT = 0) (output O, input I0, I1, I2, I3, I4);
  assign O = INIT[{I4, I3, I2, I1, I0}];
endmodule
module LUT6 #(parameter [63:0] INIT = 0) (output O, input I0, I1, I2, I3, I4, I5);
  assign O = INIT[{I5, I4, I3, I2, I1, I0}];
endmodule

// Flops: R/S synchronous, CLR/PRE asynchronous; reset wins over CE.
module FDRE #(parameter [0:0] INIT = 0, parameter [0:0] IS_C_INVERTED = 0) (output reg Q, input C, CE, R, D);
  initial Q = INIT;
  wire c = C ^ IS_C_INVERTED;
  always @(posedge c) if (R) Q <= 1'b0; else if (CE) Q <= D;
endmodule
module FDSE #(parameter [0:0] INIT = 1, parameter [0:0] IS_C_INVERTED = 0) (output reg Q, input C, CE, S, D);
  initial Q = INIT;
  wire c = C ^ IS_C_INVERTED;
  always @(posedge c) if (S) Q <= 1'b1; else if (CE) Q <= D;
endmodule
module FDCE #(parameter [0:0] INIT = 0, parameter [0:0] IS_C_INVERTED = 0) (output reg Q, input C, CE, CLR, D);
  initial Q = INIT;
  wire c = C ^ IS_C_INVERTED;
  always @(posedge c or posedge CLR) if (CLR) Q <= 1'b0; else if (CE) Q <= D;
endmodule
module FDPE #(parameter [0:0] INIT = 1, parameter [0:0] IS_C_INVERTED = 0) (output reg Q, input C, CE, PRE, D);
  initial Q = INIT;
  wire c = C ^ IS_C_INVERTED;
  always @(posedge c or posedge PRE) if (PRE) Q <= 1'b1; else if (CE) Q <= D;
endmodule

// Carry chain: carry in is CI or CYINIT; each bit propagates when S is 1,
// else generates DI; the sum is S xor the incoming carry.
module CARRY4 (output [3:0] CO, output [3:0] O, input CI, input CYINIT, input [3:0] DI, input [3:0] S);
  wire c0 = CI | CYINIT;
  assign CO[0] = S[0] ? c0 : DI[0];
  assign CO[1] = S[1] ? CO[0] : DI[1];
  assign CO[2] = S[2] ? CO[1] : DI[2];
  assign CO[3] = S[3] ? CO[2] : DI[3];
  assign O = S ^ {CO[2:0], c0};
endmodule

module MUXF7 (output O, input I0, I1, S);
  assign O = S ? I1 : I0;
endmodule
module MUXF8 (output O, input I0, I1, S);
  assign O = S ? I1 : I0;
endmodule

module SRL16E #(parameter [15:0] INIT = 0, parameter [0:0] IS_CLK_INVERTED = 0) (output Q, input A0, A1, A2, A3, CE, CLK, D);
  reg [15:0] r = INIT;
  wire c = CLK ^ IS_CLK_INVERTED;
  always @(posedge c) if (CE) r <= {r[14:0], D};
  assign Q = r[{A3, A2, A1, A0}];
endmodule
module SRLC32E #(parameter [31:0] INIT = 0, parameter [0:0] IS_CLK_INVERTED = 0) (output Q, output Q31, input [4:0] A, input CE, CLK, D);
  reg [31:0] r = INIT;
  wire c = CLK ^ IS_CLK_INVERTED;
  always @(posedge c) if (CE) r <= {r[30:0], D};
  assign Q = r[A];
  assign Q31 = r[31];
endmodule

module IBUF (output O, input I);
  assign O = I;
endmodule
module OBUF (output O, input I);
  assign O = I;
endmodule
module BUFG (output O, input I);
  assign O = I;
endmodule
module BUFGCE #(parameter SIM_DEVICE = "7SERIES") (output O, input I, CE);
  assign O = I & CE;
endmodule
// Glitch-free switching is not modelled: the selected, enabled input passes.
module BUFGCTRL #(parameter INIT_OUT = 0, parameter PRESELECT_I0 = "FALSE", parameter PRESELECT_I1 = "FALSE")
  (output O, input I0, I1, CE0, CE1, S0, S1, IGNORE0, IGNORE1);
  assign O = (S0 & CE0) ? I0 : (S1 & CE1) ? I1 : INIT_OUT[0];
endmodule

// RAMB18E1, true dual port, 18-bit ports only (16 data + 2 parity bits,
// word address ADDR[13:4]); any other width stops the simulation. Output
// registers, resets and port collisions are not modelled.
module RAMB18E1 #(
  parameter RAM_MODE = "TDP", parameter integer READ_WIDTH_A = 18, parameter integer READ_WIDTH_B = 18,
  parameter integer WRITE_WIDTH_A = 18, parameter integer WRITE_WIDTH_B = 18,
  parameter WRITE_MODE_A = "WRITE_FIRST", parameter WRITE_MODE_B = "WRITE_FIRST",
  parameter integer DOA_REG = 0, parameter integer DOB_REG = 0,
  parameter [17:0] INIT_A = 0, parameter [17:0] INIT_B = 0, parameter [17:0] SRVAL_A = 0, parameter [17:0] SRVAL_B = 0,
  parameter [255:0] INIT_00 = 0, INIT_01 = 0, INIT_02 = 0, INIT_03 = 0, INIT_04 = 0, INIT_05 = 0, INIT_06 = 0, INIT_07 = 0,
  parameter [255:0] INIT_08 = 0, INIT_09 = 0, INIT_0A = 0, INIT_0B = 0, INIT_0C = 0, INIT_0D = 0, INIT_0E = 0, INIT_0F = 0,
  parameter [255:0] INIT_10 = 0, INIT_11 = 0, INIT_12 = 0, INIT_13 = 0, INIT_14 = 0, INIT_15 = 0, INIT_16 = 0, INIT_17 = 0,
  parameter [255:0] INIT_18 = 0, INIT_19 = 0, INIT_1A = 0, INIT_1B = 0, INIT_1C = 0, INIT_1D = 0, INIT_1E = 0, INIT_1F = 0,
  parameter [255:0] INIT_20 = 0, INIT_21 = 0, INIT_22 = 0, INIT_23 = 0, INIT_24 = 0, INIT_25 = 0, INIT_26 = 0, INIT_27 = 0,
  parameter [255:0] INIT_28 = 0, INIT_29 = 0, INIT_2A = 0, INIT_2B = 0, INIT_2C = 0, INIT_2D = 0, INIT_2E = 0, INIT_2F = 0,
  parameter [255:0] INIT_30 = 0, INIT_31 = 0, INIT_32 = 0, INIT_33 = 0, INIT_34 = 0, INIT_35 = 0, INIT_36 = 0, INIT_37 = 0,
  parameter [255:0] INIT_38 = 0, INIT_39 = 0, INIT_3A = 0, INIT_3B = 0, INIT_3C = 0, INIT_3D = 0, INIT_3E = 0, INIT_3F = 0,
  parameter [255:0] INITP_00 = 0, INITP_01 = 0, INITP_02 = 0, INITP_03 = 0, INITP_04 = 0, INITP_05 = 0, INITP_06 = 0, INITP_07 = 0
) (
  input [13:0] ADDRARDADDR, ADDRBWRADDR, input CLKARDCLK, CLKBWRCLK, input [15:0] DIADI, DIBDI, input [1:0] DIPADIP, DIPBDIP,
  output reg [15:0] DOADO, DOBDO, output reg [1:0] DOPADOP, DOPBDOP, input ENARDEN, ENBWREN, REGCEAREGCE, REGCEB,
  input RSTRAMARSTRAM, RSTRAMB, RSTREGARSTREG, RSTREGB, input [1:0] WEA, input [3:0] WEBWE
);
  reg [17:0] m [0:1023];
  wire [16383:0] d = {INIT_3F, INIT_3E, INIT_3D, INIT_3C, INIT_3B, INIT_3A, INIT_39, INIT_38, INIT_37, INIT_36, INIT_35, INIT_34,
    INIT_33, INIT_32, INIT_31, INIT_30, INIT_2F, INIT_2E, INIT_2D, INIT_2C, INIT_2B, INIT_2A, INIT_29, INIT_28, INIT_27, INIT_26,
    INIT_25, INIT_24, INIT_23, INIT_22, INIT_21, INIT_20, INIT_1F, INIT_1E, INIT_1D, INIT_1C, INIT_1B, INIT_1A, INIT_19, INIT_18,
    INIT_17, INIT_16, INIT_15, INIT_14, INIT_13, INIT_12, INIT_11, INIT_10, INIT_0F, INIT_0E, INIT_0D, INIT_0C, INIT_0B, INIT_0A,
    INIT_09, INIT_08, INIT_07, INIT_06, INIT_05, INIT_04, INIT_03, INIT_02, INIT_01, INIT_00};
  wire [2047:0] dp = {INITP_07, INITP_06, INITP_05, INITP_04, INITP_03, INITP_02, INITP_01, INITP_00};
  integer i;
  initial begin
    if (READ_WIDTH_A != 18 || WRITE_WIDTH_A != 18 || READ_WIDTH_B != 18 || WRITE_WIDTH_B != 18 || DOA_REG != 0 || DOB_REG != 0) begin
      $display("RAMB18E1 model: only 18-bit ports without output registers"); $finish;
    end
    for (i = 0; i < 1024; i = i + 1) m[i] = {dp[i*2 +: 2], d[i*16 +: 16]};
    {DOPADOP, DOADO} = {INIT_A[17:16], INIT_A[15:0]};
    {DOPBDOP, DOBDO} = {INIT_B[17:16], INIT_B[15:0]};
  end
  wire [9:0] aa = ADDRARDADDR[13:4], ab = ADDRBWRADDR[13:4];
  // Memory word bit order: {parity1, parity0, data[15:0]}; byte k is data[8k+7:8k] plus parity k.
  function [17:0] merge(input [17:0] old, input [15:0] dat, input [1:0] par, input [1:0] we);
    merge = {we[1] ? par[1] : old[17], we[0] ? par[0] : old[16], we[1] ? dat[15:8] : old[15:8], we[0] ? dat[7:0] : old[7:0]};
  endfunction
  always @(posedge CLKARDCLK) if (ENARDEN) begin
    if (WRITE_MODE_A == "READ_FIRST" || WEA == 0) {DOPADOP, DOADO} <= m[aa];
    else if (WRITE_MODE_A == "WRITE_FIRST") {DOPADOP, DOADO} <= merge(m[aa], DIADI, DIPADIP, WEA);
    if (WEA != 0) m[aa] <= merge(m[aa], DIADI, DIPADIP, WEA);
  end
  always @(posedge CLKBWRCLK) if (ENBWREN) begin
    if (WRITE_MODE_B == "READ_FIRST" || WEBWE[1:0] == 0) {DOPBDOP, DOBDO} <= m[ab];
    else if (WRITE_MODE_B == "WRITE_FIRST") {DOPBDOP, DOBDO} <= merge(m[ab], DIBDI, DIPBDIP, WEBWE[1:0]);
    if (WEBWE[1:0] != 0) m[ab] <= merge(m[ab], DIBDI, DIPBDIP, WEBWE[1:0]);
  end
endmodule
