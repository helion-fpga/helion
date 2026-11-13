// 1024 x 16 block RAM whose contents come from $readmemh (with @ jumps
// and a gap that stays 0), written and read as usual.
module ram_readmem(input logic clk, input logic we, input logic [9:0] wa, input logic [9:0] ra, input logic [15:0] d, output logic [15:0] q);
  logic [15:0] mem [0:1023];
  initial $readmemh("data/ram_1k.hex", mem);
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d;
    q <= mem[ra];
  end
endmodule
