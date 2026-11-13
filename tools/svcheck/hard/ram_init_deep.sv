// 4096 x 20 (four depth banks, two slices): words 0..2999 from
// $readmemb, then words 3000.. from a loop, and two read ports.
module ram_init_deep(input logic clk, input logic we, input logic [11:0] wa, input logic [11:0] ra, input logic [11:0] rb, input logic [19:0] d,
                     output logic [19:0] qa, output logic [19:0] qb);
  logic [19:0] mem [0:4095];
  initial begin
    $readmemb("data/ram_deep.mem", mem, 0, 2999);
    for (int i = 3000; i < 4096; i++) mem[i] = 20'(i * 997);
  end
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d;
    qa <= mem[ra];
    qb <= mem[rb];
  end
endmodule
