module top(input logic clk, input logic en, output logic [3:0] q);
  logic [3:0] r = 4'd0;
  always_ff @(posedge clk) begin
    if (r != 4'd9) r <= r + 4'd1;
  end
  assign q = r;
endmodule
