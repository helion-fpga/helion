// Helion-MM PWM — period/duty regs, free-running counter, output compare (not AXI PWM).
// Width kept modest so Helion maps real LUT/FF fabric.
module h_pwm (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output wire        pwm_out
);
    // 0x00 period (N+1 counts), 0x04 duty (high while cnt < duty), 0x08 ctrl en, 0x0C count readback
    reg [7:0] period_r;
    reg [7:0] duty_r;
    reg [7:0] count_r;
    reg       en_r;
    reg       pwm_r;

    assign mm_ready = 1'b1;
    assign pwm_out  = pwm_r;

    always @(posedge clk) begin
        if (!resetn) begin
            period_r <= 8'h0F;
            duty_r   <= 8'h08;
            count_r  <= 8'h00;
            en_r     <= 1'b0;
            pwm_r    <= 1'b0;
            mm_rdata <= 32'h0;
        end else begin
            if (mm_valid && mm_write) begin
                case (mm_addr[3:0])
                    4'h0: period_r <= (mm_wdata[7:0] == 8'h0) ? 8'h01 : mm_wdata[7:0];
                    4'h4: duty_r   <= mm_wdata[7:0];
                    4'h8: begin
                        en_r <= mm_wdata[0];
                        if (mm_wdata[1]) count_r <= 8'h00;
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (mm_addr[3:0])
                    4'h0: mm_rdata <= {24'h0, period_r};
                    4'h4: mm_rdata <= {24'h0, duty_r};
                    4'h8: mm_rdata <= {31'h0, en_r};
                    4'hC: mm_rdata <= {24'h0, count_r};
                    default: mm_rdata <= 32'h0;
                endcase
            end

            if (en_r) begin
                if (count_r >= period_r)
                    count_r <= 8'h00;
                else
                    count_r <= count_r + 8'h01;
                pwm_r <= (count_r < duty_r);
            end else begin
                pwm_r <= 1'b0;
            end
        end
    end
endmodule
