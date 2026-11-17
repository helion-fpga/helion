// 65536 x 4: two RAMB36 deep per column; the registered read with an enable
// picks its row with a register of its own.
module deep (input clk, input en, input we, input [15:0] wa, input [15:0] ra, input [3:0] wd, output logic [3:0] q);
  logic [3:0] mem [0:65535];
  initial for (int i = 0; i < 65536; i++) mem[i] = 4'(i ^ (i >> 5));
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= wd;
    if (en) q <= mem[ra];
  end
endmodule
