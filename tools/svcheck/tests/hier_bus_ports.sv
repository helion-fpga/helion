// Bus ports through a hierarchy: whole bus, part-select, concat, constant.
module add8 (input logic [7:0] a, input logic [7:0] b, output logic [7:0] s);
    assign s = a + b;
endmodule
module reg8 (input logic clk, input logic en, input logic [7:0] d, output logic [7:0] q);
    always_ff @(posedge clk) if (en) q <= d;
endmodule
module top (input logic clk, input logic en, input logic [7:0] x, input logic [3:0] y, output logic [7:0] o, output logic [3:0] p);
    logic [7:0] s, r;
    logic [15:0] wide;
    add8 u_add (.a(x), .b({y, 4'b0101}), .s(s));
    reg8 u_reg (.clk(clk), .en(en), .d(s ^ 8'h3c), .q(r));
    reg8 u_hi  (.clk(clk), .en(1'b1), .d(r), .q(wide[15:8]));
    reg8 u_lo  (.clk(clk), .en(en & y[0]), .d(x), .q(wide[7:0]));
    assign o = wide[11:4] ^ r;
    assign p = wide[3:0] & wide[15:12];
endmodule
