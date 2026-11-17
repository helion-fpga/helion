// Device primitives instantiated by hand: they pass through as cells.
module prims (input clk_in, input [3:0] a, input [3:0] b, input d, input ce, input [3:0] sa,
              input we, input [9:0] wa, input [9:0] ra, input [17:0] wd,
              output [3:0] sum, output co, output q16, output [17:0] rd);
  wire clk;
  BUFG u_bufg (.I(clk_in), .O(clk));
  wire [3:0] c;
  CARRY4 u_c4 (.CI(1'b0), .CYINIT(1'b0), .DI(a), .S(a ^ b), .O(sum), .CO(c));
  assign co = c[3];
  SRL16E #(.INIT(16'hA5C3)) u_srl (.CLK(clk), .CE(ce), .D(d), .A0(sa[0]), .A1(sa[1]), .A2(sa[2]), .A3(sa[3]), .Q(q16));
  RAMB18E1 #(.READ_WIDTH_A(18), .WRITE_WIDTH_B(18), .READ_WIDTH_B(0), .WRITE_WIDTH_A(0), .WRITE_MODE_B("READ_FIRST"),
             .INIT_00(256'h0123456789ABCDEF_FEDCBA9876543210_0F1E2D3C4B5A6978_8796A5B4C3D2E1F0), .INIT_A(18'h3_0001))
    u_ram (.CLKARDCLK(clk), .ENARDEN(1'b1), .ADDRARDADDR({ra, 4'b0}), .DOADO(rd[15:0]), .DOPADOP(rd[17:16]),
           .CLKBWRCLK(clk), .ENBWREN(we), .ADDRBWRADDR({wa, 4'b0}), .DIBDI(wd[15:0]), .DIPBDIP(wd[17:16]), .WEBWE({2'b0, we, we}),
           .RSTRAMARSTRAM(1'b0), .RSTREGARSTREG(1'b0), .REGCEAREGCE(1'b0), .RSTRAMB(1'b0), .RSTREGB(1'b0), .REGCEB(1'b0));
endmodule
