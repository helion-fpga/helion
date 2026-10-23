// generate_not_lowered: genvar-indexed bit select on the LHS
module c2_gen_real (
    input  logic [3:0] b,
    output logic [3:0] y
);
    genvar i;
    generate
      for (i = 0; i < 4; i = i + 1) begin : g_bits
        assign y[i] = b[i];
      end
    endgenerate
endmodule
