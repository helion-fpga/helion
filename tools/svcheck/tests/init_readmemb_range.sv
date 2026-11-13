// $readmemb with start and end addresses into a memory declared [15:0]:
// the file fills words 11 to 13; the other words keep their declared
// value. (A start above the end loads downwards by IEEE 1800 21.4; the
// reference simulator loads upwards instead, so that case is checked by
// the front end's own tests.)
module init_readmemb_range(input logic clk, input logic [3:0] a, output logic [7:0] q);
  logic [7:0] m [15:0] = '{default: 8'h3c};
  initial begin
    $readmemb("data/rom_b.bin", m, 11, 13);
  end
  always_ff @(posedge clk) q <= m[a];
endmodule
