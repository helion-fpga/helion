// MMCME2_BASE for svcheck: x7v's primitive shim and clock model
// (tools/x7v/x7v/mmcm.py, RTL_PRIMS), so RTL and netlist run the same clocks.
`timescale 1ns/1ps

module MMCME2_BASE #(parameter BANDWIDTH = "OPTIMIZED", parameter real CLKIN1_PERIOD = 0.0, parameter real REF_JITTER1 = 0.010,
  parameter STARTUP_WAIT = "FALSE", parameter integer DIVCLK_DIVIDE = 1, parameter real CLKFBOUT_PHASE = 0.0,
  parameter real CLKFBOUT_MULT_F = 5.0,
  parameter real CLKOUT0_DIVIDE_F = 1.0, parameter real CLKOUT0_DUTY_CYCLE = 0.5, parameter real CLKOUT0_PHASE = 0.0,
  parameter integer CLKOUT1_DIVIDE = 1, parameter real CLKOUT1_DUTY_CYCLE = 0.5, parameter real CLKOUT1_PHASE = 0.0,
  parameter integer CLKOUT2_DIVIDE = 1, parameter real CLKOUT2_DUTY_CYCLE = 0.5, parameter real CLKOUT2_PHASE = 0.0,
  parameter integer CLKOUT3_DIVIDE = 1, parameter real CLKOUT3_DUTY_CYCLE = 0.5, parameter real CLKOUT3_PHASE = 0.0,
  parameter integer CLKOUT4_DIVIDE = 1, parameter real CLKOUT4_DUTY_CYCLE = 0.5, parameter real CLKOUT4_PHASE = 0.0,
  parameter integer CLKOUT5_DIVIDE = 1, parameter real CLKOUT5_DUTY_CYCLE = 0.5, parameter real CLKOUT5_PHASE = 0.0,
  parameter integer CLKOUT6_DIVIDE = 1, parameter real CLKOUT6_DUTY_CYCLE = 0.5, parameter real CLKOUT6_PHASE = 0.0,
  parameter CLKOUT4_CASCADE = "FALSE", parameter CLKFBOUT_USE_FINE_PS = "FALSE",
  parameter CLKOUT0_USE_FINE_PS = "FALSE", parameter CLKOUT1_USE_FINE_PS = "FALSE", parameter CLKOUT2_USE_FINE_PS = "FALSE", parameter CLKOUT3_USE_FINE_PS = "FALSE", parameter CLKOUT4_USE_FINE_PS = "FALSE", parameter CLKOUT5_USE_FINE_PS = "FALSE", parameter CLKOUT6_USE_FINE_PS = "FALSE")
  (input CLKIN1, input CLKFBIN, input RST, input PWRDWN, output CLKFBOUT, output CLKOUT0, output CLKOUT1, output CLKOUT2, output CLKOUT3, output CLKOUT4, output CLKOUT5, output CLKOUT6, output CLKOUT0B, output CLKOUT1B, output CLKOUT2B, output CLKOUT3B, output CLKFBOUTB, output LOCKED);
  localparam integer O8_0 = $rtoi(CLKOUT0_DIVIDE_F * 8.0 + 0.5);
  localparam integer O8_1 = $rtoi(CLKOUT1_DIVIDE * 8.0 + 0.5);
  localparam integer O8_2 = $rtoi(CLKOUT2_DIVIDE * 8.0 + 0.5);
  localparam integer O8_3 = $rtoi(CLKOUT3_DIVIDE * 8.0 + 0.5);
  localparam integer O8_4 = $rtoi(CLKOUT4_DIVIDE * 8.0 + 0.5);
  localparam integer O8_5 = $rtoi(CLKOUT5_DIVIDE * 8.0 + 0.5);
  localparam integer O8_6 = $rtoi(CLKOUT6_DIVIDE * 8.0 + 0.5);
  localparam integer HH_0 = $rtoi(CLKOUT0_DUTY_CYCLE * CLKOUT0_DIVIDE_F * 2.0 + 0.5);
  localparam integer HH_1 = $rtoi(CLKOUT1_DUTY_CYCLE * CLKOUT1_DIVIDE * 2.0 + 0.5);
  localparam integer HH_2 = $rtoi(CLKOUT2_DUTY_CYCLE * CLKOUT2_DIVIDE * 2.0 + 0.5);
  localparam integer HH_3 = $rtoi(CLKOUT3_DUTY_CYCLE * CLKOUT3_DIVIDE * 2.0 + 0.5);
  localparam integer HH_4 = $rtoi(CLKOUT4_DUTY_CYCLE * CLKOUT4_DIVIDE * 2.0 + 0.5);
  localparam integer HH_5 = $rtoi(CLKOUT5_DUTY_CYCLE * CLKOUT5_DIVIDE * 2.0 + 0.5);
  localparam integer HH_6 = $rtoi(CLKOUT6_DUTY_CYCLE * CLKOUT6_DIVIDE * 2.0 + 0.5);
  localparam integer PH8_0 = $rtoi((CLKOUT0_PHASE < 0.0 ? CLKOUT0_PHASE + 360.0 : CLKOUT0_PHASE) / 360.0 * CLKOUT0_DIVIDE_F * 8.0 + 0.5);
  localparam integer PH8_1 = $rtoi((CLKOUT1_PHASE < 0.0 ? CLKOUT1_PHASE + 360.0 : CLKOUT1_PHASE) / 360.0 * CLKOUT1_DIVIDE * 8.0 + 0.5);
  localparam integer PH8_2 = $rtoi((CLKOUT2_PHASE < 0.0 ? CLKOUT2_PHASE + 360.0 : CLKOUT2_PHASE) / 360.0 * CLKOUT2_DIVIDE * 8.0 + 0.5);
  localparam integer PH8_3 = $rtoi((CLKOUT3_PHASE < 0.0 ? CLKOUT3_PHASE + 360.0 : CLKOUT3_PHASE) / 360.0 * CLKOUT3_DIVIDE * 8.0 + 0.5);
  localparam integer PH8_4 = $rtoi((CLKOUT4_PHASE < 0.0 ? CLKOUT4_PHASE + 360.0 : CLKOUT4_PHASE) / 360.0 * CLKOUT4_DIVIDE * 8.0 + 0.5);
  localparam integer PH8_5 = $rtoi((CLKOUT5_PHASE < 0.0 ? CLKOUT5_PHASE + 360.0 : CLKOUT5_PHASE) / 360.0 * CLKOUT5_DIVIDE * 8.0 + 0.5);
  localparam integer PH8_6 = $rtoi((CLKOUT6_PHASE < 0.0 ? CLKOUT6_PHASE + 360.0 : CLKOUT6_PHASE) / 360.0 * CLKOUT6_DIVIDE * 8.0 + 0.5);
  wire [6:0] co;
  x7v_cmt #(.M8($rtoi(CLKFBOUT_MULT_F * 8.0 + 0.5)), .D(DIVCLK_DIVIDE),
    .O8({O8_6[15:0], O8_5[15:0], O8_4[15:0], O8_3[15:0], O8_2[15:0], O8_1[15:0], O8_0[15:0]}), .HH({HH_6[15:0], HH_5[15:0], HH_4[15:0], HH_3[15:0], HH_2[15:0], HH_1[15:0], HH_0[15:0]}), .PH8({PH8_6[15:0], PH8_5[15:0], PH8_4[15:0], PH8_3[15:0], PH8_2[15:0], PH8_1[15:0], PH8_0[15:0]}), .EN(7'd127))
    u (.CLKIN(CLKIN1), .RST(RST), .PWRDWN(PWRDWN), .CLKFBOUT(CLKFBOUT), .LOCKED(LOCKED), .CLKOUT(co));
  assign {CLKOUT6, CLKOUT5, CLKOUT4, CLKOUT3, CLKOUT2, CLKOUT1, CLKOUT0} = co[6:0];
endmodule

// x7v behavioural model of one MMCM/PLL, written from UG472 (not a Xilinx
// simulation model). M8: feedback multiply in eighths; D: input divide;
// per output (16-bit fields, CLKOUT0 lowest): O8 divide in eighths, HH high
// time in half VCO periods, PH8 first-edge delay in eighths of a VCO period;
// EN: enabled outputs. LOCKED rises LOCK_CYCLES CLKIN edges after RST and
// PWRDWN are low; outputs start from that edge, INSERTION ns late.
`timescale 1ns/1ps
module x7v_cmt #(parameter integer M8 = 64, parameter integer D = 1,
                 parameter [111:0] O8 = {7{16'd8}}, parameter [111:0] HH = {7{16'd1}},
                 parameter [111:0] PH8 = {7{16'd0}}, parameter [6:0] EN = 7'd0,
                 parameter integer LOCK_CYCLES = 16, parameter real INSERTION = 0.05)
  (input CLKIN, input RST, input PWRDWN, output CLKFBOUT, output reg LOCKED = 1'b0, output [6:0] CLKOUT);
  integer cnt = 0;
  realtime tprev = 0.0, tin = 0.0, tref = 0.0;
  reg run = 1'b0;
  assign CLKFBOUT = CLKIN;  // only its connection is checked (by the extractor)
  always @(posedge CLKIN or posedge RST or posedge PWRDWN) begin
    if (RST !== 1'b0 || PWRDWN !== 1'b0) begin
      cnt = 0; run = 1'b0; LOCKED <= 1'b0;
    end else begin
      if (cnt > 0) tin = $realtime - tprev;
      tprev = $realtime;
      if (cnt == LOCK_CYCLES) begin tref = $realtime; run = 1'b1; LOCKED <= 1'b1; end
      if (cnt <= LOCK_CYCLES) cnt = cnt + 1;
    end
  end
  genvar k;
  generate for (k = 0; k < 7; k = k + 1) begin : g
    localparam integer O = O8[16*k +: 16];
    localparam integer H = HH[16*k +: 16];
    localparam integer P = PH8[16*k +: 16];
    reg c = 1'b0;
    realtime tv, tn;
    assign CLKOUT[k] = c;
    if (EN[k]) begin : on
      always begin
        wait (run === 1'b1);
        tv = tin * D * 8.0 / M8;
        tn = tref + INSERTION + P * tv / 8.0;
        while (run === 1'b1) begin
          #(tn - $realtime) c = 1'b1;
          #(H * tv / 2.0) c = 1'b0;
          tn = tn + O * tv / 8.0;
        end
      end
    end
  end endgenerate
endmodule
