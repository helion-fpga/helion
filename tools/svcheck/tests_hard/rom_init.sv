// ROM from $readmemh plus a RAM filled by an initial loop; both read through address registers.
module rom_init (input clk, input [8:0] a, input we, input [8:0] wa, input [11:0] wd, output [11:0] q, output [11:0] r);
  logic [11:0] rom [0:511];
  logic [11:0] ram [0:511];
  logic [8:0] a_q;
  initial $readmemh("tools/svcheck/tests_hard/rom_init.hex", rom);
  initial for (int i = 0; i < 512; i++) ram[i] = 12'(i * 7 + 3);
  always_ff @(posedge clk) begin
    a_q <= a;
    if (we) ram[a] <= wd;
  end
  assign q = rom[a_q];
  assign r = ram[a_q];
endmodule
