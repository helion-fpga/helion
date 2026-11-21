module blinky(input clk, output led);
  reg q = 1'b0;
  always @(posedge clk) q <= ~q;
  assign led = q;
endmodule
