// 7-series clock-PIP specimen base (tcl/clkfuzz.tcl places it at random):
// a BUFG into an MMCM (CLKIN and feedback through the HCLK_CMT muxes), MMCM
// outputs to BUFGs, a BUFHCE and a BUFR, and four fabric-made clocks each on
// a BUFG feeding a BUFHCE and flops of their own. Every clock drives two flops.
module x7_clk (input wire clk, output wire o);
  wire clk_i, clk_g, fb, fb_g;
  wire [5:0] co;
  IBUFG ib (.I(clk), .O(clk_i));
  (* DONT_TOUCH = "TRUE" *) BUFG g_in (.I(clk_i), .O(clk_g));
  (* DONT_TOUCH = "TRUE" *) MMCME2_ADV #(.CLKIN1_PERIOD(10.0), .CLKFBOUT_MULT_F(10.0),
    .CLKOUT0_DIVIDE_F(10.0), .CLKOUT1_DIVIDE(10), .CLKOUT2_DIVIDE(10), .CLKOUT3_DIVIDE(10),
    .CLKOUT4_DIVIDE(10), .CLKOUT5_DIVIDE(10)) mmcm_i (
    .CLKIN1(clk_g), .CLKIN2(1'b0), .CLKINSEL(1'b1), .CLKFBIN(fb_g), .CLKFBOUT(fb),
    .CLKOUT0(co[0]), .CLKOUT1(co[1]), .CLKOUT2(co[2]), .CLKOUT3(co[3]), .CLKOUT4(co[4]), .CLKOUT5(co[5]),
    .RST(1'b0), .PWRDWN(1'b0), .PSCLK(1'b0), .PSEN(1'b0), .PSINCDEC(1'b0),
    .DCLK(1'b0), .DEN(1'b0), .DWE(1'b0), .DADDR(7'd0), .DI(16'd0));
  (* DONT_TOUCH = "TRUE" *) BUFG g_fb (.I(fb), .O(fb_g));
  wire [13:0] k;  // every clock that drives flops
  genvar i;
  generate for (i = 0; i < 4; i = i + 1) begin : m
    (* DONT_TOUCH = "TRUE" *) BUFG b (.I(co[i]), .O(k[i]));
  end endgenerate
  (* DONT_TOUCH = "TRUE" *) BUFHCE h_m (.I(co[4]), .CE(1'b1), .O(k[4]));
  (* DONT_TOUCH = "TRUE" *) BUFR #(.BUFR_DIVIDE("BYPASS")) r_m (.I(co[5]), .CE(1'b1), .CLR(1'b0), .O(k[5]));
  reg [3:0] t = 0;
  always @(posedge k[0]) t <= t + 1'b1;
  generate for (i = 0; i < 4; i = i + 1) begin : f
    (* DONT_TOUCH = "TRUE" *) BUFG b (.I(t[i]), .O(k[6 + i]));
    (* DONT_TOUCH = "TRUE" *) BUFHCE h (.I(k[6 + i]), .CE(1'b1), .O(k[10 + i]));
  end endgenerate
  wire [13:0] q;
  generate for (i = 0; i < 14; i = i + 1) begin : c
    (* DONT_TOUCH = "TRUE" *) reg [1:0] r = 0;
    always @(posedge k[i]) r <= {r[0], ~r[1]};
    assign q[i] = r[1];
  end endgenerate
  assign o = ^q;
endmodule
