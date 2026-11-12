module top(input logic clk, input logic up, output logic [3:0] q);
  logic [3:0] r = 4'd0;
  always_ff @(posedge clk) begin
    if (up) begin
      if (r != 4'd15) r <= r + 4'd1;
    end else begin
      if (r != 4'd0) r <= r - 4'd1;
    end
  end
  assign q = r;
endmodule
