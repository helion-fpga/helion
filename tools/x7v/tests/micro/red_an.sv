module top(input [63:0] a, output y);
  assign y = ~&a;
endmodule
