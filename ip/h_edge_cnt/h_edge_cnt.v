// Helion-ST rising-edge counter with clear (not AXI). Distinct from h_edge_det.
// Samples din when st_valid; increments count on 0→1; clear zeros count.
module h_edge_cnt (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST sample path
    input  wire        st_valid,
    output wire        st_ready,
    input  wire        din,
    input  wire        clear,       // level; clears count
    output reg  [15:0] count,
    output wire        rise_pulse,
    output wire        st_out_valid
);
    reg        prev_r;
    reg        have_prev;
    reg        rise_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign rise_pulse   = rise_r;
    assign st_out_valid = out_v;

    always @(posedge clk) begin
        if (!resetn) begin
            prev_r    <= 1'b0;
            have_prev <= 1'b0;
            rise_r    <= 1'b0;
            count     <= 16'h0;
            out_v     <= 1'b0;
        end else begin
            rise_r <= 1'b0;
            out_v  <= 1'b0;

            if (clear) begin
                count     <= 16'h0;
                have_prev <= 1'b0;
                prev_r    <= 1'b0;
                out_v     <= 1'b1;
            end else if (st_valid) begin
                if (have_prev && !prev_r && din) begin
                    rise_r <= 1'b1;
                    count  <= count + 16'h1;
                    out_v  <= 1'b1;
                end
                prev_r    <= din;
                have_prev <= 1'b1;
            end
        end
    end
endmodule
