// Helion-ST loadable N-bit shift register — WIDTH=32, dir/serial in/out (not AXI).
// Parallel load via load+pdata; shift on enable with dir (0=right toward LSB,
// 1=left toward MSB). serial_in feeds vacated bit; serial_out is bit shifted out.
// Real FF+LUT fabric for catalog proof.
module h_shift_reg (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST control
    input  wire        st_valid,
    output wire        st_ready,
    input  wire        enable,     // shift one step
    input  wire        load,       // parallel load pdata into shift_r
    input  wire        dir,        // 0 = right (toward LSB), 1 = left (toward MSB)
    input  wire        serial_in,  // bit shifted in
    input  wire [31:0] pdata,      // parallel load data
    output reg  [31:0] q,          // parallel register out
    output reg         serial_out, // bit shifted out
    output wire        st_out_valid
);
    reg [31:0] shift_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    always @(posedge clk) begin
        if (!resetn) begin
            shift_r    <= 32'h0;
            q          <= 32'h0;
            serial_out <= 1'b0;
            out_v      <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (load) begin
                shift_r <= pdata;
                q       <= pdata;
                out_v   <= 1'b1;
            end else if (enable || st_valid) begin
                if (!dir) begin
                    // right shift: LSB out, serial_in -> MSB
                    serial_out <= shift_r[0];
                    shift_r    <= {serial_in, shift_r[31:1]};
                    q          <= {serial_in, shift_r[31:1]};
                end else begin
                    // left shift: MSB out, serial_in -> LSB
                    serial_out <= shift_r[31];
                    shift_r    <= {shift_r[30:0], serial_in};
                    q          <= {shift_r[30:0], serial_in};
                end
                out_v <= 1'b1;
            end
        end
    end
endmodule
