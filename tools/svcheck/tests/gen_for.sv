module top #(parameter int N = 6) (input logic clk, input logic [N-1:0] d, input logic sh, output logic [N-1:0] q, output logic par);
    logic [N-1:0] stage [0:2];
    genvar i;
    generate
        for (i = 0; i < N; i++) begin : g
            always_ff @(posedge clk) begin
                stage[0][i] <= sh ? d[(i + 1) % N] : d[i];
                stage[1][i] <= stage[0][i] ^ stage[0][(i + N - 1) % N];
                stage[2][i] <= stage[1][i];
            end
        end
    endgenerate
    assign q = stage[2];
    assign par = ^stage[1];
endmodule
