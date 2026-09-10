// Helion-ST saturating add/sub — signed 8b with sticky overflow flags (not AXI).
// Loads operand A/B via Helion-ST (st_valid + sel); op=0 add, op=1 sub.
// Result saturates to 8'sh7F / 8'sh80 on signed overflow; ov_pos/ov_neg sticky
// until clear. Real FF+LUT fabric.
module h_saturate (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST load path
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire       sel,      // 0=load A, 1=load B
    input  wire       op,       // 0=add, 1=sub
    input  wire       enable,   // compute saturating result
    input  wire       clear,    // clear sticky overflow flags
    output reg  [7:0] result,
    output reg        ov_pos,   // sticky positive overflow
    output reg        ov_neg,   // sticky negative overflow
    output wire       st_out_valid
);
    reg signed [7:0] a_r;
    reg signed [7:0] b_r;
    reg              out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // 9-bit signed widen for overflow detect
    wire signed [8:0] sum9 = op ? ({a_r[7], a_r} - {b_r[7], b_r})
                                : ({a_r[7], a_r} + {b_r[7], b_r});
    wire pos_ov = (sum9 > 9'sd127);
    wire neg_ov = (sum9 < -9'sd128);
    wire signed [7:0] sat = pos_ov ? 8'sh7F : (neg_ov ? 8'sh80 : sum9[7:0]);

    always @(posedge clk) begin
        if (!resetn) begin
            a_r    <= 8'sh0;
            b_r    <= 8'sh0;
            result <= 8'h0;
            ov_pos <= 1'b0;
            ov_neg <= 1'b0;
            out_v  <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (clear) begin
                ov_pos <= 1'b0;
                ov_neg <= 1'b0;
            end
            if (st_valid) begin
                if (!sel)
                    a_r <= st_data;
                else
                    b_r <= st_data;
            end else if (enable) begin
                result <= sat;
                if (pos_ov) ov_pos <= 1'b1;
                if (neg_ov) ov_neg <= 1'b1;
                out_v <= 1'b1;
            end
        end
    end
endmodule
