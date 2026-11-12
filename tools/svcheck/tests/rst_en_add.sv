module top (input logic clk, input logic rst_n, input logic en, input logic [3:0] a, output logic [3:0] q, output logic z);
    logic [3:0] r;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) r <= 4'd5;
        else if (en) r <= r + a;
    end
    assign q = r;
    assign z = &a;
endmodule
