// Shift registers that must stay flops: one with a synchronous reset, one
// marked shreg_extract = "no", and one too short (2 stages).
module srl_keep_flops (input clk, input rst, input d, output q1, output q2, output q3);
  reg [7:0] a;
  always @(posedge clk) if (rst) a <= 0; else a <= {a[6:0], d};
  assign q1 = a[7];
  (* shreg_extract = "no" *) reg [7:0] b;
  always @(posedge clk) b <= {b[6:0], d};
  assign q2 = b[7];
  reg [1:0] c;
  always @(posedge clk) c <= {c[0], d};
  assign q3 = c[1];
endmodule
