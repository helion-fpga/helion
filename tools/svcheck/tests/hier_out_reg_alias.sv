// Child outputs that are registers or aliases of registers, read by the parent.
module cnt #(parameter int W = 4) (input logic clk, input logic rst, output logic [W-1:0] q, output logic tc);
    always_ff @(posedge clk) begin
        if (rst) q <= '0;
        else q <= q + 1'b1;
    end
    assign tc = &q;
endmodule
module top (input logic clk, input logic rst, output logic [5:0] a, output logic [2:0] b, output logic both);
    logic ta, tb;
    cnt #(.W(6)) u_a (.clk(clk), .rst(rst), .q(a), .tc(ta));
    cnt #(.W(3)) u_b (.clk(clk), .rst(rst | ta), .q(b), .tc(tb));
    assign both = ta & tb;
endmodule
