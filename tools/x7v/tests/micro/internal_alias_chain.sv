module top(input logic [3:0] a, input logic [3:0] b, output logic [3:0] y, output logic z);
  logic [3:0] au;
  logic [3:0] bu;
  logic c;
  assign au = a;
  assign bu = b[3:0];
  assign c = a[2];
  assign y = au ^ bu;
  assign z = c & bu[1];
endmodule
