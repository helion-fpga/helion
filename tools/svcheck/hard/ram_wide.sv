// A 256 x 24 memory: two Bram18 side by side (16 + 8 bits).
module ram_wide(input logic clk, input logic we, input logic [7:0] wa, input logic [7:0] ra,
                input logic [23:0] d, output logic [23:0] q);
  logic [23:0] mem [0:255] = '{0: 24'habcdef, 1: 24'h123456, 255: 24'hfedcba, default: 24'h010203};
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d;
    q <= mem[ra];
  end
endmodule
