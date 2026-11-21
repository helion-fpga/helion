module carry8inc(input clk, input clk2, output [7:0] led);
  reg [7:0] cnt = 0;
  always @(posedge clk) cnt <= cnt + 1;
  assign led = cnt;
endmodule
