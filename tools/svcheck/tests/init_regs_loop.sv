// Power-up values from initial blocks: plain assignments to registers, a
// for loop filling a RAM with a function of the index, a nonblocking
// assignment, a later initial overriding part of an earlier one, and a
// constant held by a variable nothing else drives.
module init_regs_loop(input logic clk, input logic we, input logic [3:0] wa, input logic [3:0] ra, input logic [7:0] d,
                      output logic [7:0] q, output logic [7:0] cnt_o, output logic [15:0] sh_o, output logic [5:0] k_o);
  logic [7:0] ram [16];
  logic [7:0] cnt;
  logic [15:0] sh;
  logic [5:0] k;
  function automatic logic [7:0] f(input int i);
    return 8'(i * 37 + 5) ^ 8'(i << 4);
  endfunction
  initial begin
    for (int i = 0; i < 16; i++) ram[i] = f(i);
    cnt = 8'd200;
    sh <= 16'hbeef;
    k = 6'd42;
  end
  initial ram[3] = 8'h99;
  always_ff @(posedge clk) begin
    if (we) ram[wa] <= d;
    q <= ram[ra];
    cnt <= cnt + 8'd3;
    sh <= {sh[14:0], sh[15] ^ sh[13]};
  end
  assign cnt_o = cnt;
  assign sh_o = sh;
  assign k_o = k;
endmodule
