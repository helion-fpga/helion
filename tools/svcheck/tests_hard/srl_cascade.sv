// Shift registers past 32 stages cascade SRLC32Es through Q31: a 70-stage
// fixed delay with an enable, and a 64-stage one read at a variable stage
// (two SRLC32Es and a LUT picking between them on the top address bit).
module srl_cascade (input clk, input ce, input d, input [5:0] a, output q70, output q64);
  reg [69:0] s;
  always @(posedge clk) if (ce) s <= {s[68:0], d};
  assign q70 = s[69];
  reg [63:0] t;
  always @(posedge clk) t <= {t[62:0], d ^ s[3]};
  assign q64 = t[a];
endmodule
