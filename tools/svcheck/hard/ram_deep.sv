// A 4096 x 8 memory: four Bram18 deep, each 1024 words; the bank of a
// read is registered with it and chooses the output.
module ram_deep(input logic clk, input logic we, input logic re, input logic [11:0] wa, input logic [11:0] ra,
                input logic [7:0] d, output logic [7:0] q);
  logic [7:0] mem [0:4095] = '{0: 8'h11, 1023: 8'h22, 1024: 8'h33, 2047: 8'h44, 4095: 8'h55, default: 8'h5a};
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d;
    if (re) q <= mem[ra];
  end
endmodule
