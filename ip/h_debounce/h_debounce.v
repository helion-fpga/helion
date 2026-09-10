// Helion-ST debounce — N-stage shift-register filter with stable out (not AXI).
// STAGE=8: sample noisy_in into a shift chain; stable_out updates only when all
// stages agree. Real FF+LUT fabric for catalog proof.
module h_debounce (
    input  wire clk,
    input  wire resetn,
    // Helion-ST sample enable (ingress)
    input  wire st_valid,
    output wire st_ready,
    input  wire noisy_in,
    output reg  stable_out,
    output wire stable_valid
);
    // 8-stage shift register
    reg [7:0] shift_r;
    reg       all_hi;
    reg       all_lo;
    reg       stable_valid_r;

    assign st_ready     = 1'b1;
    assign stable_valid = stable_valid_r;

    always @(*) begin
        all_hi = &shift_r;
        all_lo = ~(|shift_r);
    end

    always @(posedge clk) begin
        if (!resetn) begin
            shift_r        <= 8'h00;
            stable_out     <= 1'b0;
            stable_valid_r <= 1'b0;
        end else begin
            if (st_valid) begin
                shift_r <= {shift_r[6:0], noisy_in};
            end
            // Update stable only when all stages agree
            if (all_hi) begin
                stable_out     <= 1'b1;
                stable_valid_r <= 1'b1;
            end else if (all_lo) begin
                stable_out     <= 1'b0;
                stable_valid_r <= 1'b1;
            end else begin
                stable_valid_r <= 1'b0;
            end
        end
    end
endmodule
