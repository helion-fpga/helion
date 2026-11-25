// UltraScale+ fuzz base: one of each primitive whose config bits Helion needs.
// Every sweep variant changes one property of one of these cells after routing
// (tcl/fuzz.tcl), so the bit diff against base.bit is that property's encoding.
module us_sink (
    input        clk,
    input  [7:0] a,
    output       y,
    inout        io0
);
    wire l, q, locked, clk_g, clkfb, clk_m;
    wire [7:0] co, co_o;
    wire [47:0] p;
    wire [31:0] bd;
    wire [71:0] ud;

    (* DONT_TOUCH = "TRUE" *) LUT6 #(.INIT(64'h0)) lut_i (
        .I0(a[0]), .I1(a[1]), .I2(a[2]), .I3(a[3]), .I4(a[4]), .I5(a[5]), .O(l));
    (* DONT_TOUCH = "TRUE" *) FDRE ff_i (.C(clk_g), .CE(a[6]), .R(a[7]), .D(l), .Q(q));
    (* DONT_TOUCH = "TRUE" *) CARRY8 carry_i (
        .CI(a[0]), .CI_TOP(1'b0), .DI(a), .S(~a), .O(co_o), .CO(co));
    (* DONT_TOUCH = "TRUE" *) DSP48E2 dsp_i (
        .CLK(clk_g), .A({22'b0, a}), .B({10'b0, a}), .C({40'b0, a}), .D({19'b0, a}),
        .ALUMODE(4'b0), .INMODE(5'b0), .OPMODE(9'b000110101), .CARRYINSEL(3'b0),
        .CEA1(1'b1), .CEA2(1'b1), .CEB1(1'b1), .CEB2(1'b1), .CEC(1'b1), .CED(1'b1),
        .CEAD(1'b1), .CEM(1'b1), .CEP(1'b1), .CECTRL(1'b1), .CEINMODE(1'b1),
        .CEALUMODE(1'b1), .CECARRYIN(1'b1), .P(p));
    (* DONT_TOUCH = "TRUE" *) RAMB36E2 #(.READ_WIDTH_A(36), .READ_WIDTH_B(36),
        .WRITE_WIDTH_A(36), .WRITE_WIDTH_B(36)) bram_i (
        .CLKARDCLK(clk_g), .CLKBWRCLK(clk_g), .ENARDEN(1'b1), .ENBWREN(1'b1),
        .ADDRARDADDR({a, 7'b0}), .ADDRBWRADDR({a, 7'b0}), .DINADIN({24'b0, a}),
        .WEA(4'b0), .WEBWE({7'b0, a[0]}), .DOUTADOUT(bd));
    (* DONT_TOUCH = "TRUE" *) URAM288 uram_i (
        .CLK(clk_g), .ADDR_A({15'b0, a}), .ADDR_B({15'b0, a}), .EN_A(1'b1), .EN_B(1'b1),
        .RDB_WR_A(1'b0), .RDB_WR_B(a[0]), .BWE_A(9'h1ff), .BWE_B(9'h1ff),
        .DIN_A({64'b0, a}), .DIN_B({64'b0, a}), .DOUT_A(ud));
    (* DONT_TOUCH = "TRUE" *) MMCME4_ADV #(.CLKIN1_PERIOD(10.0), .CLKFBOUT_MULT_F(10.0),
        .CLKOUT0_DIVIDE_F(10.0)) mmcm_i (
        .CLKIN1(clk_g), .CLKFBIN(clkfb), .CLKFBOUT(clkfb), .CLKOUT0(clk_m),
        .RST(a[7]), .PWRDWN(1'b0), .CLKINSEL(1'b1), .LOCKED(locked));
    (* DONT_TOUCH = "TRUE" *) BUFGCE bufg_i (.I(clk), .CE(1'b1), .O(clk_g));
    reg m;
    always @(posedge clk_m) m <= a[3];
    (* DONT_TOUCH = "TRUE" *) IOBUF iobuf_i (.I(q), .T(a[4]), .O(), .IO(io0));
    assign y = ^{q, co, co_o, p, bd, ud, locked, m};
endmodule
