// Helion-ST Manchester encoder from bit stream + clock enable (not AXI).
// IEEE 802.3 style: data=0 → 01 (L→H mid), data=1 → 10 (H→L mid).
// Loads up to 8 pending bits via Helion-ST; on enable emits one half-bit
// per cycle (two enables per source bit). Real FF+LUT fabric.
module h_manchester_enc (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST bit-stream load (st_data bits queued MSB-first)
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire       enable,      // advance one half-bit when pending
    input  wire       init,        // clear shifter / idle
    output reg        man_out,     // Manchester serial out
    output wire       busy,
    output wire       bit_done,    // pulsed when a full source bit completes
    output wire       st_out_valid
);
    reg [7:0] shift_r;
    reg [3:0] bits_left; // 0 = idle; 1..8 = source bits remaining
    reg       half_r;    // 0=first half, 1=second half
    reg       out_v;
    reg       done_r;

    assign st_ready     = (bits_left == 4'h0);
    assign busy         = (bits_left != 4'h0);
    assign bit_done     = done_r;
    assign st_out_valid = out_v;

    wire cur_bit = shift_r[7];
    // first half = ~bit, second half = bit  (IEEE 802.3)
    wire half_val = half_r ? cur_bit : ~cur_bit;

    always @(posedge clk) begin
        if (!resetn) begin
            shift_r   <= 8'h00;
            bits_left <= 4'h0;
            half_r    <= 1'b0;
            man_out   <= 1'b0;
            out_v     <= 1'b0;
            done_r    <= 1'b0;
        end else begin
            out_v  <= 1'b0;
            done_r <= 1'b0;

            if (init) begin
                bits_left <= 4'h0;
                half_r    <= 1'b0;
                man_out   <= 1'b0;
            end else if (st_valid && (bits_left == 4'h0)) begin
                shift_r   <= st_data;
                bits_left <= 4'h8;
                half_r    <= 1'b0;
            end else if (enable && (bits_left != 4'h0)) begin
                man_out <= half_val;
                out_v   <= 1'b1;
                if (!half_r) begin
                    half_r <= 1'b1;
                end else begin
                    half_r    <= 1'b0;
                    shift_r   <= {shift_r[6:0], 1'b0};
                    bits_left <= bits_left - 4'h1;
                    done_r    <= 1'b1;
                end
            end
        end
    end
endmodule
