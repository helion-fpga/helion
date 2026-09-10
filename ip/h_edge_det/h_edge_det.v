// Helion-ST edge detector — rising/falling sticky status + clear (not AXI).
// Samples din when st_valid; compares to previous sample; sticky flags latch
// until cleared via st_clear. Real FF+LUT fabric for catalog proof.
module h_edge_det (
    input  wire clk,
    input  wire resetn,
    // Helion-ST sample path
    input  wire st_valid,
    output wire st_ready,
    input  wire din,
    // Clear sticky status (level; sampled each cycle)
    input  wire st_clear,
    output reg  rise_sticky,
    output reg  fall_sticky,
    output wire rise_pulse,
    output wire fall_pulse
);
    reg        prev_r;
    reg        rise_r;
    reg        fall_r;
    reg        have_prev;

    assign st_ready   = 1'b1;
    assign rise_pulse = rise_r;
    assign fall_pulse = fall_r;

    always @(posedge clk) begin
        if (!resetn) begin
            prev_r      <= 1'b0;
            have_prev   <= 1'b0;
            rise_r      <= 1'b0;
            fall_r      <= 1'b0;
            rise_sticky <= 1'b0;
            fall_sticky <= 1'b0;
        end else begin
            rise_r <= 1'b0;
            fall_r <= 1'b0;

            if (st_clear) begin
                rise_sticky <= 1'b0;
                fall_sticky <= 1'b0;
            end

            if (st_valid) begin
                if (have_prev) begin
                    // Rising: 0->1
                    if (!prev_r && din) begin
                        rise_r      <= 1'b1;
                        rise_sticky <= 1'b1;
                    end
                    // Falling: 1->0
                    if (prev_r && !din) begin
                        fall_r      <= 1'b1;
                        fall_sticky <= 1'b1;
                    end
                end
                prev_r    <= din;
                have_prev <= 1'b1;
            end
        end
    end
endmodule
