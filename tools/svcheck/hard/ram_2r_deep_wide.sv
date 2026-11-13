// All three at once: 2048 x 20, two read ports: 2 ports x 2 banks x 2
// slices = 8 Bram18. Descending declaration.
module ram_2r_deep_wide(input logic clk, input logic we, input logic re1, input logic [10:0] wa, input logic [10:0] ra1,
                        input logic [10:0] ra2, input logic [19:0] d, output logic [19:0] q1, output logic [19:0] q2);
  logic [19:0] mem [2047:0] = '{0: 20'h12345, 5: 20'hfedcb, 1023: 20'h0a0a0, 1024: 20'h55555, 2000: 20'h80001, 2047: 20'h7ffff, default: 20'h00f0f};
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d ^ 20'h0f0f0;
    if (re1) q1 <= mem[ra1];
    q2 <= mem[ra2];
  end
endmodule
