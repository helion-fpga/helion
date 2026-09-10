// Helion-ST registered endian byte-reverse — 32b ↔ bytes (not AXI).
// Loads 32b word via four Helion-ST bytes (st_valid + byte_sel), then on
// enable registers q = {din[7:0], din[15:8], din[23:16], din[31:24]}.
// Real FF+LUT fabric for catalog proof.
module h_byte_rev (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path (byte-wise into din)
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire [1:0]  byte_sel, // which byte of din[31:0] to load
    input  wire        enable,   // compute & register byte-reverse
    output reg  [31:0] q,        // endian-swapped result
    output wire        st_out_valid
);
    reg [31:0] din;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Combinational endian byte-reverse (wire permute → LUT/FF register)
    wire [31:0] rev = {din[7:0], din[15:8], din[23:16], din[31:24]};

    always @(posedge clk) begin
        if (!resetn) begin
            din   <= 32'h0;
            q     <= 32'h0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (byte_sel)
                    2'b00: din[7:0]   <= st_data;
                    2'b01: din[15:8]  <= st_data;
                    2'b10: din[23:16] <= st_data;
                    2'b11: din[31:24] <= st_data;
                endcase
            end else if (enable) begin
                q     <= rev;
                out_v <= 1'b1;
            end
        end
    end
endmodule
