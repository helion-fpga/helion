// A deep RAM (two banks) beside registers and a port named like the
// nets and flops made up for it: the bank select (q_bank0) and a bank's
// data out (q_bank1_do3, q_bank0_do0).
module ram_name_clash(input logic clk, input logic we, input logic t, input logic [10:0] wa, input logic [10:0] ra, input logic [7:0] d,
                      output logic [7:0] q, output logic q_bank0, output logic q_bank1_do3, output logic q_bank0_do0);
  logic [7:0] mem [0:2047];
  always_ff @(posedge clk) begin
    if (we) mem[wa] <= d;
    q <= mem[ra];
    q_bank0 <= t;
    q_bank1_do3 <= ~t;
  end
  assign q_bank0_do0 = ~t;
endmodule
