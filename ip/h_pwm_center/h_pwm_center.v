// Helion-ST center-aligned PWM — up/down triangle counter vs duty (not AXI).
// Distinct from edge-aligned h_pwm and complementary h_pwm_deadtime.
// st_valid+st_data loads period (sel=0), duty (1), ctrl en (2).
module h_pwm_center (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST config path
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire [1:0] sel,      // 0=period 1=duty 2=ctrl
    input  wire       enable,   // free-run when high (or via ctrl load)
    output reg        pwm_out,
    output wire [7:0] count_out,
    output wire       st_out_valid
);
    reg [7:0] period_r;
    reg [7:0] duty_r;
    reg [7:0] count_r;
    reg       up_r;     // 1=counting up, 0=counting down
    reg       en_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign count_out    = count_r;
    assign st_out_valid = out_v;

    wire run = en_r | enable;

    always @(posedge clk) begin
        if (!resetn) begin
            period_r <= 8'h1F;
            duty_r   <= 8'h10;
            count_r  <= 8'h00;
            up_r     <= 1'b1;
            en_r     <= 1'b0;
            pwm_out  <= 1'b0;
            out_v    <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (sel)
                    2'd0: period_r <= (st_data == 8'h0) ? 8'h01 : st_data;
                    2'd1: duty_r   <= st_data;
                    default: begin
                        en_r <= st_data[0];
                        if (st_data[1]) begin
                            count_r <= 8'h00;
                            up_r    <= 1'b1;
                        end
                    end
                endcase
            end else if (run) begin
                // Triangle: 0 → period → 0
                if (up_r) begin
                    if (count_r >= period_r) begin
                        up_r    <= 1'b0;
                        if (count_r != 8'h0)
                            count_r <= count_r - 8'h01;
                    end else begin
                        count_r <= count_r + 8'h01;
                    end
                end else begin
                    if (count_r == 8'h0) begin
                        up_r    <= 1'b1;
                        count_r <= 8'h01;
                    end else begin
                        count_r <= count_r - 8'h01;
                    end
                end
                // Center-aligned: high while count < duty
                pwm_out <= (count_r < duty_r);
                out_v   <= 1'b1;
            end else begin
                pwm_out <= 1'b0;
            end
        end
    end
endmodule
