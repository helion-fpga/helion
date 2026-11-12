module leaf (input logic [3:0] a, input logic [3:0] b, output logic [3:0] y, output logic c);
    logic [4:0] t;
    assign t = a + b;
    assign y = t[3:0];
    assign c = t[4];
endmodule
module mid (input logic clk, input logic [7:0] d, output logic [7:0] q, output logic ov);
    logic [3:0] lo, hi;
    logic c0, c1;
    leaf u_lo (.a(d[3:0]), .b(q[3:0]), .y(lo), .c(c0));
    leaf u_hi (.a(d[7:4]), .b(q[7:4]), .y(hi), .c(c1));
    always_ff @(posedge clk) q <= {hi, lo};
    assign ov = c0 ^ c1;
endmodule
module top (input logic clk, input logic [7:0] d, output logic [7:0] q0, output logic [7:0] q1, output logic ov);
    logic o0, o1;
    mid u0 (.clk(clk), .d(d), .q(q0), .ov(o0));
    mid u1 (.clk(clk), .d(q0 ^ d), .q(q1), .ov(o1));
    assign ov = o0 | o1;
endmodule
