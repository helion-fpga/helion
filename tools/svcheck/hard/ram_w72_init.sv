// 256 x 72 with an initial value: five 16-bit slices, INIT bits above
// bit 63.
module ram_w72_init(input logic clk, input logic we, input logic [7:0] wa, input logic [7:0] ra, input logic [71:0] d, output logic [71:0] q);
  logic [71:0] mem [0:255] = '{0: 72'hA5_0123456789ABCDEF, 1: 72'h80_0000000000000000, 7: 72'hFF_FFFF0000FFFF0000, 255: 72'h3C_DEADBEEFCAFEF00D, default: 72'h12_3456789ABCDEF012};
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d;
    q <= mem[ra];
  end
endmodule
