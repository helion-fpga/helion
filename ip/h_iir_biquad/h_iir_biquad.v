// Helion-ST simple DF1 biquad step — registered, small 8b coeffs (not AXI).
// y = ((b0*x + b1*x1 + b2*x2 - a1*y1 - a2*y2) >>> 7); delay lines shift.
// st_valid loads x(0)/b0(1)/b1(2)/b2(3) via st_data; a1/a2 via a_sel+a_data.
// enable performs one DF1 step. Helion-native (no Xilinx IP / AXI).
module h_iir_biquad (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path for x / b-coeffs
    input  wire        st_valid,
    input  wire [15:0] st_data,
    output wire        st_ready,
    input  wire [1:0]  sel,       // 0=x 1=b0 2=b1 3=b2
    input  wire        a_valid,   // load a-coeffs
    input  wire        a_sel,     // 0=a1 1=a2
    input  wire [7:0]  a_data,
    input  wire        enable,    // one biquad step
    input  wire        clear,     // clear delay lines
    output reg  signed [15:0] y_out,
    output wire        st_out_valid
);
    reg signed [15:0] x_r, x1_r, x2_r;
    reg signed [15:0] y1_r, y2_r;
    reg signed [7:0]  b0_r, b1_r, b2_r;
    reg signed [7:0]  a1_r, a2_r;
    reg               out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Combinational DF1 MAC sum with >>7 scale → LUT fabric
    wire signed [23:0] t0 = b0_r * x_r;
    wire signed [23:0] t1 = b1_r * x1_r;
    wire signed [23:0] t2 = b2_r * x2_r;
    wire signed [23:0] t3 = a1_r * y1_r;
    wire signed [23:0] t4 = a2_r * y2_r;
    wire signed [24:0] sum = $signed({t0[23], t0}) + $signed({t1[23], t1})
                           + $signed({t2[23], t2}) - $signed({t3[23], t3})
                           - $signed({t4[23], t4});
    wire signed [15:0] y_n = sum[22:7]; // >>> 7

    always @(posedge clk) begin
        if (!resetn) begin
            x_r   <= 16'sd0;
            x1_r  <= 16'sd0;
            x2_r  <= 16'sd0;
            y1_r  <= 16'sd0;
            y2_r  <= 16'sd0;
            b0_r  <= 8'sd64;  // ~0.5 in Q7
            b1_r  <= 8'sd0;
            b2_r  <= 8'sd0;
            a1_r  <= 8'sd0;
            a2_r  <= 8'sd0;
            y_out <= 16'sd0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (clear) begin
                x1_r  <= 16'sd0;
                x2_r  <= 16'sd0;
                y1_r  <= 16'sd0;
                y2_r  <= 16'sd0;
                y_out <= 16'sd0;
            end
            if (st_valid) begin
                case (sel)
                    2'd0: x_r  <= st_data;
                    2'd1: b0_r <= st_data[7:0];
                    2'd2: b1_r <= st_data[7:0];
                    default: b2_r <= st_data[7:0];
                endcase
                out_v <= 1'b1;
            end
            if (a_valid) begin
                if (a_sel)
                    a2_r <= a_data;
                else
                    a1_r <= a_data;
                out_v <= 1'b1;
            end
            if (enable) begin
                // DF1 delay update
                x2_r  <= x1_r;
                x1_r  <= x_r;
                y2_r  <= y1_r;
                y1_r  <= y_n;
                y_out <= y_n;
                out_v <= 1'b1;
            end
        end
    end
endmodule
