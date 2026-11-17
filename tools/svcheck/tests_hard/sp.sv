// Single port, read-first: the read is registered.
module sp (input clk, input we, input [9:0] addr, input [7:0] din, output reg [7:0] dout);
  reg [7:0] mem [0:1023];
  always @(posedge clk) begin
    if (we) mem[addr] <= din;
    dout <= mem[addr];
  end
endmodule
