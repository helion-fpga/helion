// UltraScale+ DSP48E2 fuzz base. Unlike us_sink.v every control pin (ALUMODE,
// INMODE, OPMODE, CARRYIN, all RSTs) is driven by a flip-flop, so IS_*_INVERTED
// variants are not folded into constants, and every pipeline register starts at 1
// so the sweep can take each one to 0 (or 2).
module us_dsp (
    input        clk,
    input        din,
    output       y
);
    reg [127:0] sh;
    always @(posedge clk) sh <= {sh[126:0], din};
    wire [47:0] p;
    (* DONT_TOUCH = "TRUE" *) DSP48E2 dsp_i (
        .CLK(clk),
        .A(sh[29:0]), .B(sh[47:30]), .C(sh[95:48]), .D(sh[122:96]),
        .ALUMODE(sh[3:0]), .INMODE(sh[8:4]), .OPMODE(sh[17:9]),
        .CARRYIN(sh[18]), .CARRYINSEL(3'b000),
        .CEA1(1'b1), .CEA2(1'b1), .CEB1(1'b1), .CEB2(1'b1), .CEC(1'b1), .CED(1'b1),
        .CEAD(1'b1), .CEM(1'b1), .CEP(1'b1), .CECTRL(1'b1), .CEINMODE(1'b1),
        .CEALUMODE(1'b1), .CECARRYIN(1'b1),
        .RSTA(sh[19]), .RSTALLCARRYIN(sh[20]), .RSTALUMODE(sh[21]), .RSTB(sh[22]),
        .RSTC(sh[23]), .RSTCTRL(sh[24]), .RSTD(sh[25]), .RSTINMODE(sh[26]),
        .RSTM(sh[27]), .RSTP(sh[28]),
        .P(p));
    assign y = ^p;
endmodule
