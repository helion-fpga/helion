// Single port, write-first: the read goes through an address register.
module sp_wf (input clk, input we, input [9:0] addr, input [15:0] din, output [15:0] dout);
  logic [15:0] mem [1024];
  logic [9:0] addr_q;
  always_ff @(posedge clk) begin
    if (we) mem[addr] <= din;
    addr_q <= addr;
  end
  assign dout = mem[addr_q];
endmodule
