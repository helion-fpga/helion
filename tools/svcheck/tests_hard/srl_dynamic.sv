// Shift registers read at a variable stage become addressed SRLs: 16 deep
// with an enable (SRL16E), 32 deep (SRLC32E), an ascending vector read
// from the other end (the address goes in inverted), and an 8-bit wide
// array (one SRL per bit, the address shared).
module srl_dynamic (input clk, input ce, input d, input [3:0] a4, input [4:0] a5, input [7:0] w,
                    output q16, output q32, output qr, output [7:0] qw);
  reg [15:0] s16;
  always @(posedge clk) if (ce) s16 <= {s16[14:0], d};
  assign q16 = s16[a4];
  reg [31:0] s32;
  always @(posedge clk) s32 <= {s32[30:0], d};
  assign q32 = s32[a5];
  reg [0:15] r;
  always @(posedge clk) r <= {d ^ ce, r[0:14]};
  assign qr = r[a4];
  reg [7:0] m [0:7];
  integer i;
  always @(posedge clk) if (ce) begin
    m[0] <= w;
    for (i = 1; i < 8; i = i + 1) m[i] <= m[i-1];
  end
  assign qw = m[a4[2:0]];
endmodule
