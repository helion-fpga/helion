// Root cause: generate_not_lowered
// A for-generate whose body is a plain `if (cond) begin assign ...; end`
module b_generate_assign (
    input  logic       a,
    input  logic [3:0] b,
    output logic [3:0] y
);
    genvar i;
    for (i = 0; i < 4; i = i + 1) begin : g_bits
        if (i == 0) begin
            assign y[i] = a;
        end else begin
            assign y[i] = b[i];
        end
    end
endmodule
