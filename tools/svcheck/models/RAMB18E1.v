// Behavioural model of the Xilinx 7-series RAMB18E1 block RAM for svcheck.
// Written for Helion from the documented ports and parameters (UG473, UG953);
// it is not Xilinx's simulation library. Covered: true dual port (RAM_MODE
// "TDP"), widths 1/2/4/9/18 per port (read and write widths may differ),
// byte write enables, WRITE_FIRST / READ_FIRST / NO_CHANGE, INIT_xx / INITP_xx
// contents, INIT_A/B and SRVAL_A/B (RSTRAM), no output register (DOx_REG 0).
// Not covered: SDP, ECC, cascade, DOx_REG 1, RSTREG. Writes from both ports
// to one address on one edge leave either value, as on the device.
module RAMB18E1 #(
  parameter integer READ_WIDTH_A = 0,
  parameter integer READ_WIDTH_B = 0,
  parameter integer WRITE_WIDTH_A = 0,
  parameter integer WRITE_WIDTH_B = 0,
  parameter integer DOA_REG = 0,
  parameter integer DOB_REG = 0,
  parameter WRITE_MODE_A = "WRITE_FIRST",
  parameter WRITE_MODE_B = "WRITE_FIRST",
  parameter RAM_MODE = "TDP",
  parameter SIM_COLLISION_CHECK = "ALL",
  parameter SIM_DEVICE = "7SERIES",
  parameter RDADDR_COLLISION_HWCONFIG = "DELAYED_WRITE",
  parameter RSTREG_PRIORITY_A = "RSTREG",
  parameter RSTREG_PRIORITY_B = "RSTREG",
  parameter INIT_FILE = "NONE",
  parameter [17:0] INIT_A = 18'h0,
  parameter [17:0] INIT_B = 18'h0,
  parameter [17:0] SRVAL_A = 18'h0,
  parameter [17:0] SRVAL_B = 18'h0,
  parameter [0:0] IS_CLKARDCLK_INVERTED = 1'b0,
  parameter [0:0] IS_CLKBWRCLK_INVERTED = 1'b0,
  parameter [0:0] IS_ENARDEN_INVERTED = 1'b0,
  parameter [0:0] IS_ENBWREN_INVERTED = 1'b0,
  parameter [0:0] IS_RSTRAMARSTRAM_INVERTED = 1'b0,
  parameter [0:0] IS_RSTRAMB_INVERTED = 1'b0,
  parameter [0:0] IS_RSTREGARSTREG_INVERTED = 1'b0,
  parameter [0:0] IS_RSTREGB_INVERTED = 1'b0,
  parameter [255:0] INIT_00 = 256'h0,
  parameter [255:0] INIT_01 = 256'h0,
  parameter [255:0] INIT_02 = 256'h0,
  parameter [255:0] INIT_03 = 256'h0,
  parameter [255:0] INIT_04 = 256'h0,
  parameter [255:0] INIT_05 = 256'h0,
  parameter [255:0] INIT_06 = 256'h0,
  parameter [255:0] INIT_07 = 256'h0,
  parameter [255:0] INIT_08 = 256'h0,
  parameter [255:0] INIT_09 = 256'h0,
  parameter [255:0] INIT_0A = 256'h0,
  parameter [255:0] INIT_0B = 256'h0,
  parameter [255:0] INIT_0C = 256'h0,
  parameter [255:0] INIT_0D = 256'h0,
  parameter [255:0] INIT_0E = 256'h0,
  parameter [255:0] INIT_0F = 256'h0,
  parameter [255:0] INIT_10 = 256'h0,
  parameter [255:0] INIT_11 = 256'h0,
  parameter [255:0] INIT_12 = 256'h0,
  parameter [255:0] INIT_13 = 256'h0,
  parameter [255:0] INIT_14 = 256'h0,
  parameter [255:0] INIT_15 = 256'h0,
  parameter [255:0] INIT_16 = 256'h0,
  parameter [255:0] INIT_17 = 256'h0,
  parameter [255:0] INIT_18 = 256'h0,
  parameter [255:0] INIT_19 = 256'h0,
  parameter [255:0] INIT_1A = 256'h0,
  parameter [255:0] INIT_1B = 256'h0,
  parameter [255:0] INIT_1C = 256'h0,
  parameter [255:0] INIT_1D = 256'h0,
  parameter [255:0] INIT_1E = 256'h0,
  parameter [255:0] INIT_1F = 256'h0,
  parameter [255:0] INIT_20 = 256'h0,
  parameter [255:0] INIT_21 = 256'h0,
  parameter [255:0] INIT_22 = 256'h0,
  parameter [255:0] INIT_23 = 256'h0,
  parameter [255:0] INIT_24 = 256'h0,
  parameter [255:0] INIT_25 = 256'h0,
  parameter [255:0] INIT_26 = 256'h0,
  parameter [255:0] INIT_27 = 256'h0,
  parameter [255:0] INIT_28 = 256'h0,
  parameter [255:0] INIT_29 = 256'h0,
  parameter [255:0] INIT_2A = 256'h0,
  parameter [255:0] INIT_2B = 256'h0,
  parameter [255:0] INIT_2C = 256'h0,
  parameter [255:0] INIT_2D = 256'h0,
  parameter [255:0] INIT_2E = 256'h0,
  parameter [255:0] INIT_2F = 256'h0,
  parameter [255:0] INIT_30 = 256'h0,
  parameter [255:0] INIT_31 = 256'h0,
  parameter [255:0] INIT_32 = 256'h0,
  parameter [255:0] INIT_33 = 256'h0,
  parameter [255:0] INIT_34 = 256'h0,
  parameter [255:0] INIT_35 = 256'h0,
  parameter [255:0] INIT_36 = 256'h0,
  parameter [255:0] INIT_37 = 256'h0,
  parameter [255:0] INIT_38 = 256'h0,
  parameter [255:0] INIT_39 = 256'h0,
  parameter [255:0] INIT_3A = 256'h0,
  parameter [255:0] INIT_3B = 256'h0,
  parameter [255:0] INIT_3C = 256'h0,
  parameter [255:0] INIT_3D = 256'h0,
  parameter [255:0] INIT_3E = 256'h0,
  parameter [255:0] INIT_3F = 256'h0,
  parameter [255:0] INITP_00 = 256'h0,
  parameter [255:0] INITP_01 = 256'h0,
  parameter [255:0] INITP_02 = 256'h0,
  parameter [255:0] INITP_03 = 256'h0,
  parameter [255:0] INITP_04 = 256'h0,
  parameter [255:0] INITP_05 = 256'h0,
  parameter [255:0] INITP_06 = 256'h0,
  parameter [255:0] INITP_07 = 256'h0
) (
  input [13:0] ADDRARDADDR,
  input [13:0] ADDRBWRADDR,
  input CLKARDCLK,
  input CLKBWRCLK,
  input [15:0] DIADI,
  input [15:0] DIBDI,
  input [1:0] DIPADIP,
  input [1:0] DIPBDIP,
  input ENARDEN,
  input ENBWREN,
  input REGCEAREGCE,
  input REGCEB,
  input RSTRAMARSTRAM,
  input RSTRAMB,
  input RSTREGARSTREG,
  input RSTREGB,
  input [1:0] WEA,
  input [3:0] WEBWE,
  output [15:0] DOADO,
  output [15:0] DOBDO,
  output [1:0] DOPADOP,
  output [1:0] DOPBDOP
);
  reg [16383:0] d;
  reg [2047:0] p;
  reg [35:0] qa, qb;
  integer k;
  initial begin
    d[0 +: 256] = INIT_00;
    d[256 +: 256] = INIT_01;
    d[512 +: 256] = INIT_02;
    d[768 +: 256] = INIT_03;
    d[1024 +: 256] = INIT_04;
    d[1280 +: 256] = INIT_05;
    d[1536 +: 256] = INIT_06;
    d[1792 +: 256] = INIT_07;
    d[2048 +: 256] = INIT_08;
    d[2304 +: 256] = INIT_09;
    d[2560 +: 256] = INIT_0A;
    d[2816 +: 256] = INIT_0B;
    d[3072 +: 256] = INIT_0C;
    d[3328 +: 256] = INIT_0D;
    d[3584 +: 256] = INIT_0E;
    d[3840 +: 256] = INIT_0F;
    d[4096 +: 256] = INIT_10;
    d[4352 +: 256] = INIT_11;
    d[4608 +: 256] = INIT_12;
    d[4864 +: 256] = INIT_13;
    d[5120 +: 256] = INIT_14;
    d[5376 +: 256] = INIT_15;
    d[5632 +: 256] = INIT_16;
    d[5888 +: 256] = INIT_17;
    d[6144 +: 256] = INIT_18;
    d[6400 +: 256] = INIT_19;
    d[6656 +: 256] = INIT_1A;
    d[6912 +: 256] = INIT_1B;
    d[7168 +: 256] = INIT_1C;
    d[7424 +: 256] = INIT_1D;
    d[7680 +: 256] = INIT_1E;
    d[7936 +: 256] = INIT_1F;
    d[8192 +: 256] = INIT_20;
    d[8448 +: 256] = INIT_21;
    d[8704 +: 256] = INIT_22;
    d[8960 +: 256] = INIT_23;
    d[9216 +: 256] = INIT_24;
    d[9472 +: 256] = INIT_25;
    d[9728 +: 256] = INIT_26;
    d[9984 +: 256] = INIT_27;
    d[10240 +: 256] = INIT_28;
    d[10496 +: 256] = INIT_29;
    d[10752 +: 256] = INIT_2A;
    d[11008 +: 256] = INIT_2B;
    d[11264 +: 256] = INIT_2C;
    d[11520 +: 256] = INIT_2D;
    d[11776 +: 256] = INIT_2E;
    d[12032 +: 256] = INIT_2F;
    d[12288 +: 256] = INIT_30;
    d[12544 +: 256] = INIT_31;
    d[12800 +: 256] = INIT_32;
    d[13056 +: 256] = INIT_33;
    d[13312 +: 256] = INIT_34;
    d[13568 +: 256] = INIT_35;
    d[13824 +: 256] = INIT_36;
    d[14080 +: 256] = INIT_37;
    d[14336 +: 256] = INIT_38;
    d[14592 +: 256] = INIT_39;
    d[14848 +: 256] = INIT_3A;
    d[15104 +: 256] = INIT_3B;
    d[15360 +: 256] = INIT_3C;
    d[15616 +: 256] = INIT_3D;
    d[15872 +: 256] = INIT_3E;
    d[16128 +: 256] = INIT_3F;
    p[0 +: 256] = INITP_00;
    p[256 +: 256] = INITP_01;
    p[512 +: 256] = INITP_02;
    p[768 +: 256] = INITP_03;
    p[1024 +: 256] = INITP_04;
    p[1280 +: 256] = INITP_05;
    p[1536 +: 256] = INITP_06;
    p[1792 +: 256] = INITP_07;
    qa = {18'b0, INIT_A};
    qb = {18'b0, INIT_B};
  end
  // Data and parity bits of a port width, and the lowest address bit used.
  function integer dw(input integer w); dw = w < 9 ? w : w / 9 * 8; endfunction
  function integer pw(input integer w); pw = w < 9 ? 0 : w / 9; endfunction
  function integer lo(input integer w); lo = w == 1 ? 0 : w == 2 ? 1 : w == 4 ? 2 : w == 9 ? 3 : w == 18 ? 4 : w == 36 ? 5 : 6; endfunction

  // Port A.
  integer a_ra, a_wa, a_j, a_pos;
  reg a_hit;
  reg [35:0] a_v;
  always @(posedge CLKARDCLK) if (ENARDEN) begin
    a_ra = ADDRARDADDR >> lo(READ_WIDTH_A);
    a_wa = ADDRARDADDR >> lo(WRITE_WIDTH_A);
    a_hit = 1'b0;
    for (k = 0; k < 2; k = k + 1) if (k < (WRITE_WIDTH_A < 18 ? 1 : WRITE_WIDTH_A / 9) && WEA[k]) a_hit = 1'b1;
    // The word read, with this edge's write in it for WRITE_FIRST.
    a_v = 36'b0;
    for (k = 0; k < dw(READ_WIDTH_A); k = k + 1) begin
      a_pos = a_ra * dw(READ_WIDTH_A) + k;
      a_j = a_pos - a_wa * dw(WRITE_WIDTH_A);
      if (WRITE_MODE_A == "WRITE_FIRST" && a_j >= 0 && a_j < dw(WRITE_WIDTH_A) && WEA[WRITE_WIDTH_A < 18 ? 0 : a_j / 8])
        a_v[k] = DIADI[a_j];
      else
        a_v[k] = d[a_pos];
    end
    for (k = 0; k < pw(READ_WIDTH_A); k = k + 1) begin
      a_pos = a_ra * pw(READ_WIDTH_A) + k;
      a_j = a_pos - a_wa * pw(WRITE_WIDTH_A);
      if (WRITE_MODE_A == "WRITE_FIRST" && a_j >= 0 && a_j < pw(WRITE_WIDTH_A) && WEA[WRITE_WIDTH_A < 18 ? 0 : a_j])
        a_v[dw(READ_WIDTH_A) + k] = DIPADIP[a_j];
      else
        a_v[dw(READ_WIDTH_A) + k] = p[a_pos];
    end
    if (RSTRAMARSTRAM) qa <= {18'b0, SRVAL_A};
    else if (WRITE_MODE_A != "NO_CHANGE" || !a_hit) qa <= a_v;
    // Write, after the read (non-blocking: the other port reads the old word).
    for (k = 0; k < dw(WRITE_WIDTH_A); k = k + 1)
      if (WEA[WRITE_WIDTH_A < 18 ? 0 : k / 8]) d[a_wa * dw(WRITE_WIDTH_A) + k] <= DIADI[k];
    for (k = 0; k < pw(WRITE_WIDTH_A); k = k + 1)
      if (WEA[WRITE_WIDTH_A < 18 ? 0 : k]) p[a_wa * pw(WRITE_WIDTH_A) + k] <= DIPADIP[k];
  end
  assign DOADO = qa & ((36'h1 << dw(READ_WIDTH_A)) - 1);
  assign DOPADOP = (qa >> dw(READ_WIDTH_A)) & ((36'h1 << pw(READ_WIDTH_A)) - 1);

  // Port B.
  integer b_ra, b_wa, b_j, b_pos;
  reg b_hit;
  reg [35:0] b_v;
  always @(posedge CLKBWRCLK) if (ENBWREN) begin
    b_ra = ADDRBWRADDR >> lo(READ_WIDTH_B);
    b_wa = ADDRBWRADDR >> lo(WRITE_WIDTH_B);
    b_hit = 1'b0;
    for (k = 0; k < 2; k = k + 1) if (k < (WRITE_WIDTH_B < 18 ? 1 : WRITE_WIDTH_B / 9) && WEBWE[k]) b_hit = 1'b1;
    // The word read, with this edge's write in it for WRITE_FIRST.
    b_v = 36'b0;
    for (k = 0; k < dw(READ_WIDTH_B); k = k + 1) begin
      b_pos = b_ra * dw(READ_WIDTH_B) + k;
      b_j = b_pos - b_wa * dw(WRITE_WIDTH_B);
      if (WRITE_MODE_B == "WRITE_FIRST" && b_j >= 0 && b_j < dw(WRITE_WIDTH_B) && WEBWE[WRITE_WIDTH_B < 18 ? 0 : b_j / 8])
        b_v[k] = DIBDI[b_j];
      else
        b_v[k] = d[b_pos];
    end
    for (k = 0; k < pw(READ_WIDTH_B); k = k + 1) begin
      b_pos = b_ra * pw(READ_WIDTH_B) + k;
      b_j = b_pos - b_wa * pw(WRITE_WIDTH_B);
      if (WRITE_MODE_B == "WRITE_FIRST" && b_j >= 0 && b_j < pw(WRITE_WIDTH_B) && WEBWE[WRITE_WIDTH_B < 18 ? 0 : b_j])
        b_v[dw(READ_WIDTH_B) + k] = DIPBDIP[b_j];
      else
        b_v[dw(READ_WIDTH_B) + k] = p[b_pos];
    end
    if (RSTRAMB) qb <= {18'b0, SRVAL_B};
    else if (WRITE_MODE_B != "NO_CHANGE" || !b_hit) qb <= b_v;
    // Write, after the read (non-blocking: the other port reads the old word).
    for (k = 0; k < dw(WRITE_WIDTH_B); k = k + 1)
      if (WEBWE[WRITE_WIDTH_B < 18 ? 0 : k / 8]) d[b_wa * dw(WRITE_WIDTH_B) + k] <= DIBDI[k];
    for (k = 0; k < pw(WRITE_WIDTH_B); k = k + 1)
      if (WEBWE[WRITE_WIDTH_B < 18 ? 0 : k]) p[b_wa * pw(WRITE_WIDTH_B) + k] <= DIPBDIP[k];
  end
  assign DOBDO = qb & ((36'h1 << dw(READ_WIDTH_B)) - 1);
  assign DOPBDOP = (qb >> dw(READ_WIDTH_B)) & ((36'h1 << pw(READ_WIDTH_B)) - 1);

endmodule
