// Shift registers with a fixed tap become SRLs (HELION_V2_HARD): a 12-stage
// delay with an enable (SRL16E), a 4-bit wide 20-stage delay (SRLC32E per
// bit), and a chain with a tap in the middle, which splits it in two.
module srl_fixed (input clk, input ce, input d, input [3:0] w, output q12, output [3:0] q20,
                  output mid, output last);
  reg [11:0] a;
  always @(posedge clk) if (ce) a <= {a[10:0], d};
  assign q12 = a[11];
  reg [3:0] b [0:19];
  integer i;
  always @(posedge clk) begin
    b[0] <= w;
    for (i = 1; i < 20; i = i + 1) b[i] <= b[i-1];
  end
  assign q20 = b[19];
  reg [9:0] c;
  always @(posedge clk) c <= {c[8:0], d ^ ce};
  assign mid = c[4];
  assign last = c[9];
endmodule
