// Products as combinational DSP48E1s: the operand shapes that fit the
// 25 x 18 signed multiplier, and some that stay in the fabric.
module dsp_comb (
    input  logic signed [24:0] sa,
    input  logic signed [17:0] sb,
    input  logic        [23:0] ua,
    input  logic        [16:0] ub,
    input  logic        [17:0] u18a, u18b,
    input  logic        [15:0] x16, y16,
    input  logic signed [19:0] s20,
    input  logic signed [11:0] s12,
    output logic signed [42:0] p_ss,     // signed 25 x 18
    output logic        [40:0] p_uu,     // unsigned 24 x 17
    output logic        [47:0] p_mix,    // signed * unsigned is unsigned: 20 x 17 zero-extended
    output logic        [15:0] p_trunc,  // 16 x 16 truncated to 16 bits
    output logic signed [63:0] p_wide,   // result wider than the product and than P
    output logic signed [42:0] p_swap,   // 18 x 25: the wide operand goes on A
    output logic        [35:0] p_u18,    // unsigned 18 x 18 needs 19 x 19 signed: fabric
    output logic        [31:0] p_k100,   // constant with three set bits: DSP
    output logic        [31:0] p_k3,     // constant 3 is one adder: fabric
    output logic signed [31:0] p_kneg,   // negative constant
    output logic        [24:0] p_self    // the same product twice: one DSP
);
    assign p_ss    = sa * sb;
    assign p_uu    = ua * ub;
    assign p_mix   = s20 * ub;
    assign p_trunc = x16 * y16;
    assign p_wide  = s20 * s12;
    assign p_swap  = sb * sa;
    assign p_u18   = u18a * u18b;
    assign p_k100  = x16 * 100;
    assign p_k3    = x16 * 3;
    assign p_kneg  = s20 * -12345;
    assign p_self  = (x16 * y16) ^ {x16, 9'd0};
endmodule
