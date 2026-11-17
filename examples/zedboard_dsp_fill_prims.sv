// examples/zedboard_dsp_fill.sv with an explicit DSP48E1 per lane.
//
// Same lanes, fold, signature, LEDs and EXP_SIG as zedboard_dsp_fill.sv
// (read its header); only the multiply-accumulate is written as a device
// primitive instead of `acc <= acc + a * b`, so it builds through the v2
// front end's primitive pass-through before `*` is inferred as a DSP.
//
// Each DSP48E1 (UG479) is set up as a signed 25x18 multiply-accumulate:
//   AREG = BREG = 1 (A2/B2), MREG = 1, PREG = 1; no pre-adder, no C or D;
//   INMODE = 00000 (A2 * B2), OPMODE = 010_01_01 (Z = P, Y = X = M),
//   ALUMODE = 0000 (P <= Z + X + Y + CIN), CARRYIN = 0;
//   RSTA/RSTB/RSTM/RSTP = restart (synchronous), all register enables 1.
// That is a <= S[24:0], b <= S[31:14], m <= a * b, P <= P + m, all cleared
// by restart: the same registers as the inferred design.
//
// Build (Helion only):
//   HELION_V2_HARD=1 HELION_XILINX_DB=~/.cache/helion/prjxray-db \
//   cargo run --release -p helion-xilinx7 --example zedbit -- \
//       examples/zedboard_dsp_fill_prims.sv examples/zedboard_dsp_fill.xdc \
//       dsp_fill.bit --front=v2
module zedboard_dsp_fill_prims #(
    parameter int          LANES   = 192,
    parameter int          NCYC    = 1024,
    parameter logic [31:0] EXP_SIG = 32'h201901BD
) (
    input  logic       clk,
    input  logic       rst,
    output logic [7:0] led
);
    localparam int GRP = 6;                     // words XORed in one LUT level
    localparam int NG  = (LANES + GRP - 1) / GRP;
    localparam int NG2 = (NG + GRP - 1) / GRP;
    localparam logic [31:0] POLY = 32'h04C11DB7;

    // LFSR seed of lane `l` (never zero).
    function automatic logic [31:0] seed(input int unsigned l);
        logic [31:0] h;
        h = (l + 1) * 32'h9E3779B1;
        h = h ^ (h >> 16);
        h = h * 32'h85EBCA6B;
        h = h ^ (h >> 13);
        return h | 32'd1;
    endfunction

    function automatic logic [47:0] rotl48(input logic [47:0] v, input int r);
        logic [95:0] w;
        w = {v, v} << r;
        return w[95:48];
    endfunction

    // Power-on reset: flops start at zero when the board is configured.
    logic [3:0] por = 4'd0;
    always_ff @(posedge clk) if (por != 4'd15) por <= por + 4'd1;
    logic [1:0] rst_sync = 2'b00;
    always_ff @(posedge clk) rst_sync <= {rst_sync[0], rst};
    wire restart = (por != 4'd15) | rst_sync[1];

    logic [48*LANES-1:0] accrot;

    for (genvar l = 0; l < LANES; l++) begin : g_lane
        logic [31:0] lfsr;
        logic [47:0] acc;
        always_ff @(posedge clk) lfsr <= restart ? seed(l) : {lfsr[30:0], 1'b0} ^ (lfsr[31] ? POLY : 32'd0);
        DSP48E1 #(
            .A_INPUT("DIRECT"), .B_INPUT("DIRECT"), .USE_DPORT("FALSE"), .USE_MULT("MULTIPLY"),
            .USE_SIMD("ONE48"), .USE_PATTERN_DETECT("NO_PATDET"), .AUTORESET_PATDET("NO_RESET"),
            .AREG(1), .BREG(1), .ACASCREG(1), .BCASCREG(1), .MREG(1), .PREG(1),
            .ADREG(0), .CREG(0), .DREG(0), .INMODEREG(0), .OPMODEREG(0), .ALUMODEREG(0),
            .CARRYINREG(0), .CARRYINSELREG(0)
        ) u_mac (
            .CLK(clk),
            .A({{5{lfsr[24]}}, lfsr[24:0]}), .B(lfsr[31:14]), .C(48'd0), .D(25'd0),
            .INMODE(5'b00000), .OPMODE(7'b0100101), .ALUMODE(4'b0000),
            .CARRYIN(1'b0), .CARRYINSEL(3'b000),
            .ACIN(30'd0), .BCIN(18'd0), .PCIN(48'd0), .CARRYCASCIN(1'b0), .MULTSIGNIN(1'b0),
            .CEA1(1'b1), .CEA2(1'b1), .CEB1(1'b1), .CEB2(1'b1), .CEM(1'b1), .CEP(1'b1),
            .CEAD(1'b0), .CEC(1'b0), .CED(1'b0), .CECTRL(1'b0), .CEINMODE(1'b0),
            .CEALUMODE(1'b0), .CECARRYIN(1'b0),
            .RSTA(restart), .RSTB(restart), .RSTM(restart), .RSTP(restart),
            .RSTC(1'b0), .RSTD(1'b0), .RSTCTRL(1'b0), .RSTINMODE(1'b0),
            .RSTALUMODE(1'b0), .RSTALLCARRYIN(1'b0),
            .P(acc),
            .ACOUT(), .BCOUT(), .PCOUT(), .CARRYCASCOUT(), .CARRYOUT(), .MULTSIGNOUT(),
            .OVERFLOW(), .UNDERFLOW(), .PATTERNDETECT(), .PATTERNBDETECT()
        );
        assign accrot[48*l +: 48] = rotl48(acc, l % 48);
    end

    // Fold pipeline: acc -> s1 -> s2 -> x -> sig, each stage XORing at most
    // GRP words (a chain of XORs maps one LUT per GRP).
    logic [48*NG-1:0]  fold1, s1;
    logic [48*NG2-1:0] fold2, s2;
    logic [47:0]       fold3, x;

    always_comb begin
        fold1 = '0;
        for (int l = 0; l < LANES; l++)
            fold1[48*(l/GRP) +: 48] = fold1[48*(l/GRP) +: 48] ^ accrot[48*l +: 48];
        fold2 = '0;
        for (int g = 0; g < NG; g++)
            fold2[48*(g/GRP) +: 48] = fold2[48*(g/GRP) +: 48] ^ s1[48*g +: 48];
        fold3 = '0;
        for (int g = 0; g < NG2; g++)
            fold3 = fold3 ^ s2[48*g +: 48];
    end

    // Controller: cnt counts clocks from restart; acc is sampled while
    // cnt < NCYC and the signature is checked once the pipeline drains.
    localparam int CW = $clog2(NCYC + 4);
    localparam logic [CW-1:0] CHECK = CW'(NCYC + 3);
    logic [CW-1:0] cnt  = '0;
    logic          v1 = 1'b0, v2 = 1'b0, v3 = 1'b0;
    logic [31:0]   sig  = 32'd0;
    logic          done = 1'b0;
    logic          fail = 1'b0;

    always_ff @(posedge clk) begin
        v1 <= ~restart & ~done & (cnt < CW'(NCYC));
        v2 <= v1 & ~restart;
        v3 <= v2 & ~restart;
        s1 <= fold1;
        s2 <= fold2;
        x  <= fold3;
        if (restart) begin
            cnt  <= '0;
            sig  <= 32'd0;
            done <= 1'b0;
            fail <= 1'b0;
        end else if (!done) begin
            if (v3) sig <= {sig[30:0], 1'b0} ^ (sig[31] ? POLY : 32'd0) ^ x[31:0] ^ {16'd0, x[47:32]};
            if (cnt == CHECK) begin
                fail <= sig != EXP_SIG;
                done <= 1'b1;
            end
            cnt <= cnt + 1'b1;
        end
    end

    logic [26:0] beat = 27'd0;
    always_ff @(posedge clk) beat <= beat + 27'd1;

    wire ok = done & ~fail;
    assign led = {done, ok, 5'b00000, ok ? beat[26] : fail};
endmodule
