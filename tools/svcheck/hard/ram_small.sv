// A 64 x 12 memory declared descending, written under two conditions and
// read every cycle at the address just written (read-first).
module ram_small(input logic clk, input logic en, input logic [1:0] mode, input logic [5:0] a,
                 input logic [11:0] d, output logic [11:0] q);
  logic [11:0] mem [63:0];
  always_ff @(posedge clk) begin
    if (en) begin
      if (mode == 2'd1) mem[a] <= d;
      else if (mode == 2'd2) mem[a] <= ~d;
    end
    q <= mem[a];
  end
endmodule
