// The file name is a string parameter of a submodule; x and z digits load
// as 0; a word wider than the memory keeps its low bits; words 2 and 3
// are left as declared.
module init_readmem_param_rom #(parameter FILE = "none.hex", parameter W = 16) (input logic [2:0] a, output logic [W-1:0] y);
  logic [W-1:0] m [0:7] = '{default: '1};
  initial $readmemh(FILE, m, 0, 3);
  assign y = m[a];
endmodule
module init_readmem_param(input logic clk, input logic [2:0] a, output logic [15:0] q, output logic [11:0] r);
  logic [15:0] y;
  init_readmem_param_rom #(.FILE("data/rom_xz.hex"), .W(16)) u0(.a(a), .y(y));
  init_readmem_param_rom #(.FILE("data/rom_xz.hex"), .W(12)) u1(.a(a), .y(r));
  always_ff @(posedge clk) q <= y;
endmodule
