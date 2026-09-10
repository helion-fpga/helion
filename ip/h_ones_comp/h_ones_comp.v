// Helion-ST registered ones-complement / invert+inc negate (not AXI).
// Loads A via Helion-ST bytes (st_valid + byte_sel); on enable registers
// q = mode? (~a + 1) : (~a) — twos-complement negate vs ones-complement.
// Real FF+LUT fabric.
module h_ones_comp (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path (byte-wise into A)
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire [1:0]  byte_sel, // which byte of A
    input  wire        mode,     // 0=ones-comp (~a), 1=negate (~a+1)
    input  wire        enable,   // compute & register result
    output reg  [31:0] q,
    output wire        st_out_valid
);
    reg [31:0] a_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Invert + optional +1 → LUT fabric
    wire [31:0] inv = ~a_r;
    wire [32:0] neg33 = {1'b0, inv} + 33'h1;
    wire [31:0] res = mode ? neg33[31:0] : inv;

    always @(posedge clk) begin
        if (!resetn) begin
            a_r   <= 32'h0;
            q     <= 32'h0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (byte_sel)
                    2'b00: a_r[7:0]   <= st_data;
                    2'b01: a_r[15:8]  <= st_data;
                    2'b10: a_r[23:16] <= st_data;
                    2'b11: a_r[31:24] <= st_data;
                endcase
            end else if (enable) begin
                q     <= res;
                out_v <= 1'b1;
            end
        end
    end
endmodule
