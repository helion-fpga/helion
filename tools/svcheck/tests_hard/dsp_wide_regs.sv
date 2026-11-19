// Products wider than one DSP48E1 at more widths (64 x 64, 33 x 17, 25 x 25,
// 24 x 18 unsigned), and registered around them: the slice registers stay
// unused (a partial product cannot be delayed against the sum that reads
// it), so operand and product registers, enables, resets and an
// accumulator stay in the fabric.
module dsp_wide_regs (
    input  logic                clk, rst, en,
    input  logic signed [63:0]  sa, sb,
    input  logic        [32:0]  u33,
    input  logic        [16:0]  u17,
    input  logic signed [24:0]  s25a, s25b,
    input  logic        [23:0]  u24,
    input  logic        [17:0]  u18,
    output logic signed [127:0] p_s64,  // signed 64 x 64
    output logic        [49:0]  p_u33,  // unsigned 33 x 17
    output logic signed [49:0]  p_s25,  // signed 25 x 25
    output logic        [41:0]  p_u24,  // unsigned 24 x 18 (25 x 19 signed)
    output logic signed [63:0]  q_mul,  // registered operands and product, enable, reset
    output logic        [63:0]  q_acc   // unsigned 32 x 32 accumulated into 64 bits
);
    logic signed [31:0] ra, rb;
    logic        [31:0] ua, ub;
    assign p_s64 = sa * sb;
    assign p_u33 = u33 * u17;
    assign p_s25 = s25a * s25b;
    assign p_u24 = u24 * u18;
    always_ff @(posedge clk) begin
        if (rst) begin
            ra <= 0; rb <= 0; q_mul <= 0; ua <= 0; ub <= 0; q_acc <= 0;
        end else begin
            if (en) begin
                ra <= sa[31:0]; rb <= sb[63:32]; q_mul <= ra * rb;
            end
            ua <= u33[31:0]; ub <= {u17, u24[14:0]};
            q_acc <= q_acc + ua * ub;
        end
    end
endmodule
