// Helion-ST serial-in parallel-out (SIPO) shift register — WIDTH=16 (not AXI).
// Shift serial_in on enable; after WIDTH shifts assert done + parallel q.
// Distinct from loadable bidirectional h_shift_reg. Real FF+LUT fabric.
module h_sipo (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST control
    input  wire        st_valid,
    output wire        st_ready,
    input  wire        enable,     // shift one bit in
    input  wire        clear,      // clear shift/count
    input  wire        serial_in,  // bit shifted toward MSB
    output reg  [15:0] q,          // parallel out
    output reg  [3:0]  bit_count,  // bits captured (0..16)
    output reg         done,       // sticky: WIDTH bits captured
    output wire        st_out_valid
);
    reg [15:0] shift_r;
    reg [3:0]  cnt_r;
    reg        done_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    always @(posedge clk) begin
        if (!resetn) begin
            shift_r   <= 16'h0;
            cnt_r     <= 4'h0;
            done_r    <= 1'b0;
            q         <= 16'h0;
            bit_count <= 4'h0;
            done      <= 1'b0;
            out_v     <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (clear) begin
                shift_r   <= 16'h0;
                cnt_r     <= 4'h0;
                done_r    <= 1'b0;
                q         <= 16'h0;
                bit_count <= 4'h0;
                done      <= 1'b0;
                out_v     <= 1'b1;
            end else if (enable || st_valid) begin
                if (!done_r) begin
                    shift_r <= {shift_r[14:0], serial_in};
                    cnt_r   <= cnt_r + 4'h1;
                    if (cnt_r == 4'hF) begin
                        done_r <= 1'b1;
                        done   <= 1'b1;
                        q      <= {shift_r[14:0], serial_in};
                    end
                    bit_count <= cnt_r + 4'h1;
                end
                out_v <= 1'b1;
            end
        end
    end
endmodule
