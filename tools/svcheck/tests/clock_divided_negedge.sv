module top(input logic clk, input logic [2:0] d, output logic [3:0] slow, output logic [2:0] fall, output logic [2:0] rise);
  logic [1:0] div;
  logic [3:0] s;
  logic [2:0] r, f;
  always_ff @(posedge clk) begin
    div <= div + 2'd1;
    r <= d;
  end
  always_ff @(negedge clk) f <= r ^ d;
  always_ff @(posedge div[1]) s <= s + {1'b0, d};
  assign slow = s; assign fall = f; assign rise = r;
endmodule
