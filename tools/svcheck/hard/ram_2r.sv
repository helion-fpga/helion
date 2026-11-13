// A 512 x 16 memory read at two addresses: two Bram18 holding the same
// contents, both written, one per read port.
module ram_2r(input logic clk, input logic we, input logic re2, input logic [8:0] wa, input logic [8:0] ra1,
              input logic [8:0] ra2, input logic [15:0] d, output logic [15:0] q1, output logic [15:0] q2);
  logic [15:0] mem [0:511] = '{3: 16'h0bad, 511: 16'hcafe, default: 16'h0000};
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d;
    q1 <= mem[ra1];
    if (re2) q2 <= mem[ra2];
  end
endmodule
