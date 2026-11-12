module inc4 (input logic clk, output logic msb);
    logic [3:0] cnt;
    always_ff @(posedge clk) cnt <= cnt + 1;
    assign msb = cnt[3];
endmodule
module xor4 (input logic a, input logic b, input logic c, input logic d, output logic y);
    assign y = ((a ^ b) ^ (c ^ d));
endmodule
module top (input logic clk, output logic led);
    logic [3:0] m;
    inc4 u00 (.clk(clk), .msb(m[0]));
    inc4 u01 (.clk(clk), .msb(m[1]));
    inc4 u02 (.clk(clk), .msb(m[2]));
    inc4 u03 (.clk(clk), .msb(m[3]));
    xor4 r0 (.a(m[0]), .b(m[1]), .c(m[2]), .d(m[3]), .y(led));
endmodule
