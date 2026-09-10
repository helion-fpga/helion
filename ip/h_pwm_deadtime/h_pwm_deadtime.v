// Helion-ST complementary PWM with programmable deadtime counters (not AXI).
// Free-run cnt vs period/duty; deadtime inserts off-gap before pwm_h/pwm_l assert.
// st_valid+st_data loads period (sel=0), duty (1), deadtime (2), ctrl en (3).
module h_pwm_deadtime (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST config path
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire [1:0] sel,      // 0=period 1=duty 2=deadtime 3=ctrl
    input  wire       enable,   // free-run when high (or via ctrl load)
    output reg        pwm_h,    // high-side
    output reg        pwm_l,    // low-side (complementary)
    output wire       st_out_valid
);
    reg [7:0] period_r;
    reg [7:0] duty_r;
    reg [7:0] dead_r;
    reg [7:0] count_r;
    reg [7:0] dt_cnt_r;
    reg       en_r;
    reg       ideal_h;   // compare result before deadtime
    reg       phase_r;   // 0=want high-side, 1=want low-side
    reg       out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    wire run = en_r | enable;

    always @(posedge clk) begin
        if (!resetn) begin
            period_r <= 8'h1F;
            duty_r   <= 8'h10;
            dead_r   <= 8'h02;
            count_r  <= 8'h00;
            dt_cnt_r <= 8'h00;
            en_r     <= 1'b0;
            ideal_h  <= 1'b0;
            phase_r  <= 1'b0;
            pwm_h    <= 1'b0;
            pwm_l    <= 1'b0;
            out_v    <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (sel)
                    2'd0: period_r <= (st_data == 8'h0) ? 8'h01 : st_data;
                    2'd1: duty_r   <= st_data;
                    2'd2: dead_r   <= st_data;
                    default: begin
                        en_r <= st_data[0];
                        if (st_data[1]) begin
                            count_r  <= 8'h00;
                            dt_cnt_r <= 8'h00;
                        end
                    end
                endcase
            end else if (run) begin
                if (count_r >= period_r)
                    count_r <= 8'h00;
                else
                    count_r <= count_r + 8'h01;

                // Ideal high while count < duty
                if (count_r < duty_r) begin
                    if (!ideal_h) begin
                        // rising edge of ideal → insert deadtime before pwm_h
                        dt_cnt_r <= dead_r;
                        phase_r  <= 1'b0;
                        pwm_h    <= 1'b0;
                        pwm_l    <= 1'b0;
                    end else if (dt_cnt_r != 8'h0) begin
                        dt_cnt_r <= dt_cnt_r - 8'h01;
                        pwm_h    <= 1'b0;
                        pwm_l    <= 1'b0;
                    end else begin
                        pwm_h <= 1'b1;
                        pwm_l <= 1'b0;
                    end
                    ideal_h <= 1'b1;
                end else begin
                    if (ideal_h) begin
                        // falling edge of ideal → insert deadtime before pwm_l
                        dt_cnt_r <= dead_r;
                        phase_r  <= 1'b1;
                        pwm_h    <= 1'b0;
                        pwm_l    <= 1'b0;
                    end else if (dt_cnt_r != 8'h0) begin
                        dt_cnt_r <= dt_cnt_r - 8'h01;
                        pwm_h    <= 1'b0;
                        pwm_l    <= 1'b0;
                    end else begin
                        pwm_h <= 1'b0;
                        pwm_l <= 1'b1;
                    end
                    ideal_h <= 1'b0;
                end
                out_v <= 1'b1;
            end else begin
                pwm_h <= 1'b0;
                pwm_l <= 1'b0;
            end
        end
    end
endmodule
