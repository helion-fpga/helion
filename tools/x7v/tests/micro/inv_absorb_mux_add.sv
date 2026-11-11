// lut_opt inverter absorption: bit 0 of `a + 1` (an inverter LUT) feeding a mux.
module top(input logic s, input logic [3:0] a, output logic [3:0] y);
  wire [3:0] t;
  assign t = a + 4'b0001;
  assign y = s ? a : t;
endmodule
