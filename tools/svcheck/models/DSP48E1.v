// Helion's behavioural model of DSP48E1 for svcheck, written from UG479
// (not a Xilinx simulation model): the behaviour of x7v's model
// (tools/x7v/x7v/dsp.py, x7v_dsp48e1) behind the primitive's own ports and
// parameters. Every register is 0 at configuration; resets are synchronous
// and win over clock enables. Not modelled: cascade inputs selected by
// A_INPUT/B_INPUT = "CASCADE", the pattern detector, the IS_*_INVERTED
// parameters, and MACC extend (OPMODE Z = 100), which read X.
module DSP48E1 #(
  parameter integer AREG = 1, ACASCREG = 1, BREG = 1, BCASCREG = 1, CREG = 1, DREG = 1, ADREG = 1,
  parameter integer MREG = 1, PREG = 1, CARRYINREG = 1, CARRYINSELREG = 1, INMODEREG = 1,
  parameter integer OPMODEREG = 1, ALUMODEREG = 1,
  parameter A_INPUT = "DIRECT", B_INPUT = "DIRECT", USE_DPORT = "FALSE", USE_MULT = "MULTIPLY",
  parameter USE_SIMD = "ONE48", USE_PATTERN_DETECT = "NO_PATDET", AUTORESET_PATDET = "NO_RESET"
)(
  input CLK,
  input [29:0] A, ACIN, input [17:0] B, BCIN, input [47:0] C, PCIN, input [24:0] D,
  input CARRYIN, CARRYCASCIN, MULTSIGNIN, input [2:0] CARRYINSEL, input [6:0] OPMODE, input [3:0] ALUMODE, input [4:0] INMODE,
  input CEA1, CEA2, CEB1, CEB2, CEC, CED, CEAD, CEM, CEP, CECARRYIN, CECTRL, CEINMODE, CEALUMODE,
  input RSTA, RSTB, RSTC, RSTD, RSTM, RSTP, RSTALLCARRYIN, RSTCTRL, RSTINMODE, RSTALUMODE,
  output [47:0] P, PCOUT, output [29:0] ACOUT, output [17:0] BCOUT, output [3:0] CARRYOUT, output CARRYCASCOUT,
  output MULTSIGNOUT, OVERFLOW, UNDERFLOW, PATTERNDETECT, PATTERNBDETECT
);
  localparam integer LANES = USE_SIMD == "FOUR12" ? 4 : USE_SIMD == "TWO24" ? 2 : 1;
  localparam integer W = 48 / LANES;
  assign {MULTSIGNOUT, OVERFLOW, UNDERFLOW, PATTERNDETECT, PATTERNBDETECT} = 5'bxxxxx;
  // control registers
  reg [4:0] im_r = 0; reg [6:0] om_r = 0; reg [3:0] al_r = 0; reg [2:0] cs_r = 0;
  always @(posedge CLK) if (RSTINMODE) im_r <= 0; else if (CEINMODE) im_r <= INMODE;
  always @(posedge CLK) if (RSTCTRL) begin om_r <= 0; cs_r <= 0; end
                        else if (CECTRL) begin om_r <= OPMODE; cs_r <= CARRYINSEL; end
  always @(posedge CLK) if (RSTALUMODE) al_r <= 0; else if (CEALUMODE) al_r <= ALUMODE;
  wire [4:0] im = INMODEREG ? im_r : INMODE;
  wire [6:0] om = OPMODEREG ? om_r : OPMODE;
  wire [3:0] al = ALUMODEREG ? al_r : ALUMODE;
  wire [2:0] cs = CARRYINSELREG ? cs_r : CARRYINSEL;
  // A and B: two-deep pipelines; a single register (xREG = 1) is the second one.
  wire [29:0] a_in = A_INPUT == "CASCADE" ? 30'bx : A;
  wire [17:0] b_in = B_INPUT == "CASCADE" ? 18'bx : B;
  reg [29:0] a1 = 0, a2 = 0; reg [17:0] b1 = 0, b2 = 0;
  wire [29:0] a1q = AREG == 2 ? a1 : a_in;
  wire [17:0] b1q = BREG == 2 ? b1 : b_in;
  always @(posedge CLK) if (RSTA) begin a1 <= 0; a2 <= 0; end else begin
    if (CEA1) a1 <= a_in;
    if (CEA2) a2 <= a1q;
  end
  always @(posedge CLK) if (RSTB) begin b1 <= 0; b2 <= 0; end else begin
    if (CEB1) b1 <= b_in;
    if (CEB2) b2 <= b1q;
  end
  wire [29:0] a2q = AREG == 0 ? a_in : a2;
  wire [17:0] b2q = BREG == 0 ? b_in : b2;
  assign ACOUT = (AREG == 2 && ACASCREG == 1) ? a1 : a2q;
  assign BCOUT = (BREG == 2 && BCASCREG == 1) ? b1 : b2q;
  // INMODE: [0] A1 not A2, [1] zero A, [2] D into the pre-adder, [3] subtract A, [4] B1 not B2.
  wire [24:0] a25 = im[1] ? 25'd0 : (im[0] ? a1q[24:0] : a2q[24:0]);
  wire [17:0] bm = im[4] ? b1q : b2q;
  reg [24:0] d_r = 0, ad_r = 0;
  always @(posedge CLK) if (RSTD) d_r <= 0; else if (CED) d_r <= D;
  wire [24:0] dq = DREG ? d_r : D;
  wire [24:0] ad = (im[2] ? dq : 25'd0) + (im[3] ? -a25 : a25);
  always @(posedge CLK) if (RSTD) ad_r <= 0; else if (CEAD) ad_r <= ad;
  wire [24:0] am = USE_DPORT == "TRUE" ? (ADREG ? ad_r : ad) : a25;
  // 25 x 18 signed multiply; the rounding bit A[24] XNOR B[17] rides with M.
  wire signed [42:0] m = $signed(am) * $signed(bm);
  reg [42:0] m_r = 0; reg rnd_r = 0;
  always @(posedge CLK) if (RSTM) m_r <= 0; else if (CEM) m_r <= m;
  always @(posedge CLK) if (RSTALLCARRYIN) rnd_r <= 0; else if (CEM) rnd_r <= ~(am[24] ^ bm[17]);
  wire [42:0] mq = MREG ? m_r : m;
  wire rnd = MREG ? rnd_r : ~(am[24] ^ bm[17]);
  reg [47:0] c_r = 0;
  always @(posedge CLK) if (RSTC) c_r <= 0; else if (CEC) c_r <= C;
  wire [47:0] cq = CREG ? c_r : C;
  // P register (with its carries); P feedback always comes from it.
  reg [47:0] p_r = 0; reg [3:0] co_r = 0;
  // X / Y / Z multiplexers. M is one 48-bit value on X; Y adds 0 for it.
  reg [47:0] xm, ym, zm;
  always @* begin
    case (om[1:0]) 2'b00: xm = 0; 2'b01: xm = {{5{mq[42]}}, mq}; 2'b10: xm = p_r; default: xm = {a2q, b2q}; endcase
    case (om[3:2]) 2'b00: ym = 0; 2'b01: ym = 0; 2'b10: ym = {48{1'b1}}; default: ym = cq; endcase
    if ((om[1:0] == 2'b01) != (om[3:2] == 2'b01)) begin xm = {48{1'bx}}; ym = {48{1'bx}}; end
    case (om[6:4])
      3'b000: zm = 0; 3'b001: zm = PCIN; 3'b010: zm = p_r; 3'b011: zm = cq;
      3'b101: zm = {{17{PCIN[47]}}, PCIN[47:17]}; 3'b110: zm = {{17{p_r[47]}}, p_r[47:17]};
      default: zm = {48{1'bx}};  // 100 MACC extend (not modelled), 111 illegal
    endcase
  end
  // carry in
  reg cin_r = 0;
  always @(posedge CLK) if (RSTALLCARRYIN) cin_r <= 0; else if (CECARRYIN) cin_r <= CARRYIN;
  reg cin;
  always @* case (cs)
    3'd0: cin = CARRYINREG ? cin_r : CARRYIN;
    3'd1: cin = ~PCIN[47];
    3'd2: cin = CARRYCASCIN;
    3'd3: cin = PCIN[47];
    3'd4: cin = co_r[3];
    3'd5: cin = ~p_r[47];
    3'd6: cin = rnd;
    default: cin = p_r[47];
  endcase
  // ALU: ALUMODE[0] inverts Z, ALUMODE[1] inverts the result.
  wire [47:0] zi = al[0] ? ~zm : zm;
  reg [47:0] sum; reg [3:0] co; reg [49:0] t;
  integer l;
  always @* begin
    sum = 0; co = 4'bxxxx;
    for (l = 0; l < LANES; l = l + 1) begin
      t = zi[l*W +: W] + xm[l*W +: W] + ym[l*W +: W] + (l == 0 ? cin : 1'b0);
      sum[l*W +: W] = t[W-1:0];
      co[(l + 1) * 4 / LANES - 1] = t[W];
    end
  end
  wire [47:0] lg = al[3] ? (ym == 0 ? (xm & zi) : ym == {48{1'b1}} ? (xm | zi) : {48{1'bx}}) : (xm ^ ym ^ zi);
  wire [47:0] r0 = al[3:2] == 2'b10 ? {48{1'bx}} : al[2] ? lg : sum;
  wire [47:0] alu = al[1] ? ~r0 : r0;
  wire [3:0] alu_co = (al[3:2] == 2'b00 && al[1] == al[0]) ? co : 4'bxxxx;
  always @(posedge CLK) if (RSTP) begin p_r <= 0; co_r <= 0; end
                        else if (CEP) begin p_r <= alu; co_r <= alu_co; end
  assign P = PREG ? p_r : alu;
  assign PCOUT = P;
  assign CARRYOUT = PREG ? co_r : alu_co;
  assign CARRYCASCOUT = CARRYOUT[3];
endmodule
