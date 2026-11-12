module top(input logic clk, input logic a, input logic b, output logic y, output logic [3:0] q);
  logic done = 1'b0;
  logic [3:0] r = 4'd0;
  always_ff @(posedge clk) begin
    done <= 1'b0;
    if (a && !b) done <= 1'b1;
    r <= 4'd3;
    if (a) r <= r + 4'd1;
    else if (b) r <= 4'd9;
  end
  assign y = done;
  assign q = r;
endmodule
