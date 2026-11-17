// True dual port, byte enables on port A; B never writes where A does.
module tdp (input clk, input [3:0] bea, input web, input [9:0] aa, input [9:0] ab,
            input [31:0] da, input [31:0] db, output logic [31:0] qa, output logic [31:0] qb);
  logic [31:0] mem [0:1023];
  always_ff @(posedge clk) begin
    for (int i = 0; i < 4; i++) if (bea[i]) mem[aa][8*i +: 8] <= da[8*i +: 8];
    qa <= mem[aa];
  end
  always_ff @(posedge clk) begin
    if (web && ab != aa) mem[ab] <= db;
    qb <= mem[ab];
  end
endmodule
