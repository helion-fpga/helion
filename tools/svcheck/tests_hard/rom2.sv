// 65536 x 2 dual-read ROM: two rows of RAMB36; one read through an address
// register, one registered.
module rom2 (input clk, input [15:0] a, input [15:0] b, output [1:0] qa, output logic [1:0] qb);
  logic [1:0] rom [0:65535];
  logic [15:0] a_q;
  initial for (int i = 0; i < 65536; i++) rom[i] = 2'((i * 37) ^ (i >> 9));
  always_ff @(posedge clk) begin
    a_q <= a;
    qb <= rom[b];
  end
  assign qa = rom[a_q];
endmodule
