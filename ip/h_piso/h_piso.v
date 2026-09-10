// Helion-ST parallel-in serial-out (PISO) shift register — WIDTH=16 (not AXI).
// load+pdata captures parallel word; enable shifts MSB-first onto serial_out.
// Distinct from loadable bidirectional h_shift_reg. Real FF+LUT fabric.
module h_piso (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST control
    input  wire        st_valid,
    output wire        st_ready,
    input  wire        enable,     // shift one bit out
    input  wire        load,       // parallel load pdata
    input  wire [15:0] pdata,      // parallel load data
    output reg         serial_out, // MSB-first serial bit
    output reg  [3:0]  bit_count,  // bits remaining (0..16)
    output reg         busy,       // sticky until emptied
    output wire        st_out_valid
);
    reg [15:0] shift_r;
    reg [3:0]  cnt_r;
    reg        busy_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    always @(posedge clk) begin
        if (!resetn) begin
            shift_r    <= 16'h0;
            cnt_r      <= 4'h0;
            busy_r     <= 1'b0;
            serial_out <= 1'b0;
            bit_count  <= 4'h0;
            busy       <= 1'b0;
            out_v      <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (load) begin
                shift_r    <= pdata;
                cnt_r      <= 4'h0;
                busy_r     <= 1'b1;
                busy       <= 1'b1;
                serial_out <= pdata[15];
                bit_count  <= 4'h0;
                out_v      <= 1'b1;
            end else if ((enable || st_valid) && busy_r) begin
                // shift left: next MSB becomes serial_out
                serial_out <= shift_r[14];
                shift_r    <= {shift_r[14:0], 1'b0};
                cnt_r      <= cnt_r + 4'h1;
                bit_count  <= cnt_r + 4'h1;
                if (cnt_r == 4'hF) begin
                    busy_r <= 1'b0;
                    busy   <= 1'b0;
                end
                out_v <= 1'b1;
            end
        end
    end
endmodule
