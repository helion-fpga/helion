// A ROM loaded with $readmemh (comments, @addresses, underscores; words
// 4..7 and 11 come from @ jumps, word 15 from the last line), read both
// combinationally and through a register.
module init_readmemh(input logic clk, input logic [3:0] a, input logic [3:0] b, output logic [11:0] q, output logic [11:0] y);
  logic [11:0] rom [0:15];
  initial $readmemh("data/rom_a.hex", rom);
  assign y = rom[b];
  always_ff @(posedge clk) q <= rom[a];
endmodule
