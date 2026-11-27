// 7-series fuzz base: one MMCME2_ADV, 100 MHz in, every output used, integer
// settings (CLKFBOUT_MULT_F 10, CLKOUT0_DIVIDE_F 10) so each sweep row moves
// one counter into its fractional or phase encoding.
module x7_mmcm (input wire clk, output wire o);
  wire clk_i, fb, fb_g;
  wire [6:0] co;
  IBUFG ib (.I(clk), .O(clk_i));
  MMCME2_ADV #(
    .CLKIN1_PERIOD(10.0), .DIVCLK_DIVIDE(1), .CLKFBOUT_MULT_F(10.0), .CLKFBOUT_PHASE(0.0),
    .CLKOUT0_DIVIDE_F(10.0), .CLKOUT1_DIVIDE(10), .CLKOUT2_DIVIDE(10), .CLKOUT3_DIVIDE(10),
    .CLKOUT4_DIVIDE(10), .CLKOUT5_DIVIDE(10), .CLKOUT6_DIVIDE(10), .BANDWIDTH("OPTIMIZED")
  ) mmcm_i (
    .CLKIN1(clk_i), .CLKIN2(1'b0), .CLKINSEL(1'b1), .CLKFBIN(fb_g), .CLKFBOUT(fb),
    .CLKOUT0(co[0]), .CLKOUT1(co[1]), .CLKOUT2(co[2]), .CLKOUT3(co[3]),
    .CLKOUT4(co[4]), .CLKOUT5(co[5]), .CLKOUT6(co[6]),
    .RST(1'b0), .PWRDWN(1'b0), .PSCLK(1'b0), .PSEN(1'b0), .PSINCDEC(1'b0),
    .DCLK(1'b0), .DEN(1'b0), .DWE(1'b0), .DADDR(7'd0), .DI(16'd0));
  BUFG bfb (.I(fb), .O(fb_g));
  reg [6:0] t = 0;
  genvar i;
  generate for (i = 0; i < 7; i = i + 1) begin : g
    wire c;
    BUFG b (.I(co[i]), .O(c));
    always @(posedge c) t[i] <= ~t[i];
  end endgenerate
  assign o = ^t;
endmodule
