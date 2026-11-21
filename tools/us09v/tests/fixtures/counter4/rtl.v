module counter4(input clk, output [3:0] led);
  reg [3:0] cnt = 0;
  always @(posedge clk) cnt <= cnt + 1;
  assign led = cnt;
endmodule
