module ch(input logic a, output logic y);
  assign y = a;
endmodule
module top(input logic b, input logic c, output logic y);
  ch u(.a(~b & c), .y(y));
endmodule
