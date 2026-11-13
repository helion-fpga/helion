// A 1024 x 16 RAM declared [0:1023] with initial contents: written at one
// address, read (registered, with an enable) at another. With
// HELION_V2_HARD=1 it becomes a Bram18.
module ram_1k(input logic clk, input logic we, input logic re, input logic [9:0] wa, input logic [9:0] ra,
              input logic [15:0] d, output logic [15:0] q, output logic [7:0] mix);
  logic [15:0] mem [0:1023] = '{0: 16'h1234, 1: 16'hbeef, 5: 16'h0042, 1022: 16'h8001, 1023: 16'h7777, default: 16'h0101};
  logic [15:0] dout;
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d;
    if (re) dout <= mem[ra];
  end
  assign q = dout;
  assign mix = dout[15:8] ^ dout[7:0] ^ wa[7:0];
endmodule
