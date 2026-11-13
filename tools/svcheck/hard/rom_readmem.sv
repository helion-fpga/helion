// A 2048 x 16 ROM from $readmemh (two depth banks of block RAM, no
// writes) read through two registers, and a 16 x 12 ROM read through one.
module rom_readmem(input logic clk, input logic [10:0] a, input logic [10:0] b, input logic [3:0] c, output logic [15:0] qa, output logic [15:0] qb, output logic [11:0] qc);
  logic [15:0] rom [0:2047];
  logic [11:0] tiny [0:15];
  initial begin
    $readmemh("data/ram_1k.hex", rom);
    $readmemh("data/rom_hi.hex", rom, 1024);
    $readmemh("../tests/data/rom_a.hex", tiny);
  end
  always_ff @(posedge clk) begin
    qa <= rom[a];
    qb <= rom[b];
    qc <= tiny[c];
  end
endmodule
