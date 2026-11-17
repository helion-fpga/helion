// 4096 x 32 with contents: four RAMB36 deep, output chosen by a registered row.
module big (input clk, input en, input we, input [11:0] wa, input [11:0] ra, input [31:0] wd, output logic [31:0] q);
  logic [31:0] mem [0:4095];
  initial for (int i = 0; i < 4096; i++) mem[i] = i * 32'h9E3779B1;
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= wd;
    if (en) q <= mem[ra];
  end
endmodule
