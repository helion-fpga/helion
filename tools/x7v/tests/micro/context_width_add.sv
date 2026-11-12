module top(input logic [3:0] a, input logic [3:0] b, output logic [7:0] s, output logic [7:0] m);
  assign s = a + b;
  assign m = a * b;
endmodule
