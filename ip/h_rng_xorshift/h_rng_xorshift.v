// Helion-ST registered xorshift32 PRNG with seed/enable (not AXI).
// Classic Marsaglia xorshift32: x ^= x<<13; x ^= x>>17; x ^= x<<5.
// st_valid + byte_sel loads seed bytes; enable advances PRNG. Real FF+LUT fabric.
module h_rng_xorshift (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST seed path
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire [1:0]  byte_sel, // which byte of 32b seed
    input  wire        enable,   // advance xorshift
    output reg  [31:0] rand_out,
    output wire        st_out_valid
);
    reg [31:0] state_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // xorshift32 combinational step → LUT fabric
    wire [31:0] x1 = state_r ^ (state_r << 13);
    wire [31:0] x2 = x1 ^ (x1 >> 17);
    wire [31:0] x3 = x2 ^ (x2 << 5);
    // Zero-lock guard: never allow all-zero state
    wire [31:0] next_state = (x3 == 32'h0) ? 32'h1 : x3;

    // Seed assemble for MSB-byte write (force non-zero)
    wire [31:0] seed_msb = {st_data, state_r[23:0]};
    wire [31:0] seed_msb_nz = (seed_msb == 32'h0) ? 32'h1 : seed_msb;

    always @(posedge clk) begin
        if (!resetn) begin
            state_r  <= 32'h0000ACE1; // non-zero default seed
            rand_out <= 32'h0000ACE1;
            out_v    <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (byte_sel)
                    2'b00: state_r[7:0]   <= st_data;
                    2'b01: state_r[15:8]  <= st_data;
                    2'b10: state_r[23:16] <= st_data;
                    2'b11: state_r        <= seed_msb_nz;
                endcase
            end else if (enable) begin
                state_r  <= next_state;
                rand_out <= next_state;
                out_v    <= 1'b1;
            end
        end
    end
endmodule
