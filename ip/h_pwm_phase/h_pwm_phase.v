// Helion-ST phase-shifted dual PWM — shared counter, two compares (not AXI).
// Distinct from edge h_pwm, complementary h_pwm_deadtime, center h_pwm_center.
// st_valid+st_data loads period (sel=0), duty (1), phase (2), ctrl en (3).
// pwm_a: count < duty; pwm_b: (count - phase) wrapped < duty.
module h_pwm_phase (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST config path
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire [1:0] sel,      // 0=period 1=duty 2=phase 3=ctrl
    input  wire       enable,   // free-run when high (or via ctrl load)
    output reg        pwm_a,
    output reg        pwm_b,
    output wire [7:0] count_out,
    output wire       st_out_valid
);
    reg [7:0] period_r;
    reg [7:0] duty_r;
    reg [7:0] phase_r;
    reg [7:0] count_r;
    reg       en_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign count_out    = count_r;
    assign st_out_valid = out_v;

    wire run = en_r | enable;

    // Phase-shifted count: subtract phase with wrap under period
    wire [7:0] cnt_b = (count_r >= phase_r)
                     ? (count_r - phase_r)
                     : (count_r + period_r + 8'h1 - phase_r);

    always @(posedge clk) begin
        if (!resetn) begin
            period_r <= 8'h1F;
            duty_r   <= 8'h10;
            phase_r  <= 8'h08;
            count_r  <= 8'h00;
            en_r     <= 1'b0;
            pwm_a    <= 1'b0;
            pwm_b    <= 1'b0;
            out_v    <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (sel)
                    2'd0: period_r <= (st_data == 8'h0) ? 8'h01 : st_data;
                    2'd1: duty_r   <= st_data;
                    2'd2: phase_r  <= st_data;
                    default: begin
                        en_r <= st_data[0];
                        if (st_data[1])
                            count_r <= 8'h00;
                    end
                endcase
            end else if (run) begin
                if (count_r >= period_r)
                    count_r <= 8'h00;
                else
                    count_r <= count_r + 8'h01;
                pwm_a <= (count_r < duty_r);
                pwm_b <= (cnt_b < duty_r);
                out_v <= 1'b1;
            end else begin
                pwm_a <= 1'b0;
                pwm_b <= 1'b0;
            end
        end
    end
endmodule
