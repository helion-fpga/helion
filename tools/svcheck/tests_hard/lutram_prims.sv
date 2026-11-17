// Every 7-series LUT RAM primitive, instantiated by hand, each with its own
// contents and read addresses taken from different bits of `ra`.
module lutram_prims (input clk, input we, input [7:0] wa, input [7:0] d, input [7:0] ra,
                     output [7:0] q32m, output [3:0] q64m, output [1:0] q32d, output [1:0] q64d,
                     output [1:0] q128d, output q256s, output q32s, output q64s, output q128s);
  RAM32M #(.INIT_A(64'h0123_4567_89AB_CDEF), .INIT_B(64'hFEDC_BA98_7654_3210),
           .INIT_C(64'h0F1E_2D3C_4B5A_6978), .INIT_D(64'h8796_A5B4_C3D2_E1F0))
    u_32m (.DOA(q32m[1:0]), .DOB(q32m[3:2]), .DOC(q32m[5:4]), .DOD(q32m[7:6]),
           .ADDRA(ra[4:0]), .ADDRB({ra[1:0], ra[4:2]}), .ADDRC(ra[7:3]), .ADDRD(wa[4:0]),
           .DIA(d[1:0]), .DIB(d[3:2]), .DIC(d[5:4]), .DID(d[7:6]), .WCLK(clk), .WE(we));
  RAM64M #(.INIT_A(64'hDEAD_BEEF_0BAD_F00D), .INIT_B(64'h1357_9BDF_2468_ACE0),
           .INIT_C(64'hA5A5_5A5A_C3C3_3C3C), .INIT_D(64'h0F0F_F0F0_6969_9696))
    u_64m (.DOA(q64m[0]), .DOB(q64m[1]), .DOC(q64m[2]), .DOD(q64m[3]),
           .ADDRA(ra[5:0]), .ADDRB({ra[2:0], ra[5:3]}), .ADDRC(ra[7:2]), .ADDRD(wa[5:0]),
           .DIA(d[0]), .DIB(d[1]), .DIC(d[2]), .DID(d[3]), .WCLK(clk), .WE(we));
  RAM32X1D #(.INIT(32'hB4C7_19E2))
    u_32d (.SPO(q32d[0]), .DPO(q32d[1]), .A0(wa[0]), .A1(wa[1]), .A2(wa[2]), .A3(wa[3]), .A4(wa[4]),
           .DPRA0(ra[3]), .DPRA1(ra[4]), .DPRA2(ra[5]), .DPRA3(ra[6]), .DPRA4(ra[7]), .D(d[4]), .WCLK(clk), .WE(we));
  RAM64X1D #(.INIT(64'h7E81_C33C_A55A_0FF0))
    u_64d (.SPO(q64d[0]), .DPO(q64d[1]), .A0(wa[0]), .A1(wa[1]), .A2(wa[2]), .A3(wa[3]), .A4(wa[4]), .A5(wa[5]),
           .DPRA0(ra[2]), .DPRA1(ra[3]), .DPRA2(ra[4]), .DPRA3(ra[5]), .DPRA4(ra[6]), .DPRA5(ra[7]), .D(d[5]), .WCLK(clk), .WE(we));
  RAM128X1D #(.INIT(128'h0011_2233_4455_6677_8899_AABB_CCDD_EEFF))
    u_128d (.SPO(q128d[0]), .DPO(q128d[1]), .A(wa[6:0]), .DPRA(ra[7:1]), .D(d[6]), .WCLK(clk), .WE(we));
  RAM256X1S #(.INIT(256'h0123_4567_89AB_CDEF_1032_5476_98BA_DCFE_2301_6745_AB89_EFCD_3210_7654_BA98_FEDC))
    u_256s (.O(q256s), .A(wa), .D(d[7]), .WCLK(clk), .WE(we));
  RAM32X1S #(.INIT(32'h9C3E_57A1))
    u_32s (.O(q32s), .A0(wa[0]), .A1(wa[1]), .A2(wa[2]), .A3(wa[3]), .A4(wa[4]), .D(d[1]), .WCLK(clk), .WE(we));
  RAM64X1S #(.INIT(64'h2B7E_1516_28AE_D2A6))
    u_64s (.O(q64s), .A0(wa[1]), .A1(wa[2]), .A2(wa[3]), .A3(wa[4]), .A4(wa[5]), .A5(wa[6]), .D(d[2]), .WCLK(clk), .WE(we));
  RAM128X1S #(.INIT(128'hF0E1_D2C3_B4A5_9687_7869_5A4B_3C2D_1E0F))
    u_128s (.O(q128s), .A0(wa[1]), .A1(wa[2]), .A2(wa[3]), .A3(wa[4]), .A4(wa[5]), .A5(wa[6]), .A6(wa[7]), .D(d[3]), .WCLK(clk), .WE(we));
endmodule
