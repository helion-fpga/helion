// Products wider than one DSP48E1 multiplier: split into partial products
// of chunks that fit (24 unsigned bits on A, 17 on B, top chunks signed),
// summed in the fabric. Signed, unsigned, mixed, truncated and constant.
module dsp_wide (
    input  logic        [31:0] ua, ub,
    input  logic signed [31:0] sa, sb,
    input  logic        [17:0] u18a, u18b,
    input  logic signed [25:0] s26,
    input  logic signed [18:0] s19,
    input  logic        [19:0] u20a, u20b,
    input  logic        [47:0] u48a, u48b,
    input  logic signed [34:0] s35a, s35b,
    output logic        [63:0] p_uu64,   // RISC-V MULHU: unsigned 32 x 32
    output logic signed [63:0] p_ss64,   // MULH: signed 32 x 32
    output logic signed [63:0] p_su64,   // MULHSU: signed x unsigned
    output logic        [31:0] p_mul32,  // MUL: the low 32 bits only
    output logic        [35:0] p_u18,    // unsigned 18 x 18 (19 x 19 signed)
    output logic signed [44:0] p_s26,    // signed 26 x 19, just over both ports
    output logic        [39:0] p_u20,    // unsigned 20 x 20
    output logic        [95:0] p_u48,    // unsigned 48 x 48
    output logic signed [69:0] p_s35,    // signed 35 x 35
    output logic        [31:0] p_k,      // 32 bits times a 32-bit constant
    output logic signed [63:0] p_edge    // operands near the extremes
);
    logic signed [31:0] ea, eb;
    assign ea = {sa[31], {31{sa[30]}}};  // 0, -1, 2^31 - 1 or -2^31
    assign eb = {sb[31], {31{sb[30]}}};
    assign p_uu64  = ua * ub;
    assign p_ss64  = sa * sb;
    assign p_su64  = $signed({{32{sa[31]}}, sa}) * $signed({32'd0, ub});
    assign p_mul32 = ua * ub;
    assign p_u18   = u18a * u18b;
    assign p_s26   = s26 * s19;
    assign p_u20   = u20a * u20b;
    assign p_u48   = u48a * u48b;
    assign p_s35   = s35a * s35b;
    assign p_k     = ua * 32'h9E3779B9;
    assign p_edge  = ea * eb;
endmodule
