// Multiplies (signed with sign extension, unsigned, mixed widths), a
// multiply-accumulate, subtracts and compares: carry chains and the
// Wallace-tree multiplier.
module mul_cmp_chain (
    input  logic        clk,
    input  logic [16:0] a,
    input  logic [16:0] b,
    input  logic [7:0]  c,
    input  logic [33:0] acc,
    output logic [33:0] smul,
    output logic [15:0] umul,
    output logic [33:0] mac,
    output logic [11:0] diff,
    output logic [3:0]  cmp,
    output logic [33:0] r
);
  assign smul = $signed({{17{a[16]}}, a}) * $signed({{17{b[16]}}, b});
  assign umul = a[7:0] * c;
  assign mac  = $signed(a) * $signed(b) + $signed(acc);
  assign diff = a[11:0] - {4'd0, c};
  assign cmp  = {a < b, $signed(a) < $signed(b), a[7:0] <= c, a == b};
  always_ff @(posedge clk) r <= r + smul - {26'd0, c};
endmodule
